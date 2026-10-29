//! Standalone audio voice daemon: performs the generative score chord slot
//! by chord slot and subscribes to subit game events on the hub — one-shot
//! combat moments become quantised `/midi/play` actions for the MIDI bridge,
//! while the streamed telemetry conducts the score (aggro locks) or
//! modulates it directly (player speed forwards immediately as `/midi/cc`
//! so the synth filter tracks motion in real time).
//!
//! The scheduler wakes at each harmonic change point, reads the latest lock
//! count and renders the next chord. An aggro change mid-chord recalculates
//! the harmonic rhythm: the change point moves earlier (a cut — the
//! sounding bass and pad are silenced at the new boundary with
//! `/midi/note-off`) when the new period lands inside the slot, and stays
//! put when it lands past the note's end.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use audio_voice::composer::{
    EventTallies, PlanRequest, ReplanReason, ScoreContext, SlotRecord, Tally,
};
use audio_voice::curve::Curve;
use audio_voice::form::Form;
use audio_voice::monitor::Monitor;
use audio_voice::snapshot::{EventLogEntry, SnapshotInputs, UiSnapshot, EVENT_LOG_LEN};
use rand::SeedableRng;
use rand::rngs::SmallRng;
use audio_voice::music::{
    self, advance_cycle_tiles, change_point, current_slot_period_secs, retune, tie_window, Engine,
    HarmonySource, MusicState, CHANNEL_AIRY, CHANNEL_BASS, CHANNEL_DISTANT, CHANNEL_PATTERN_B,
    CROTCHET_SECS, QUAVER_SECS, STUB_BPM,
};
use audio_voice::{GameEvent, EXPRESSION_CC, MOD_WHEEL_CC, NoteEvent, Quantiser, Subdivision, perform_event};
use ensemble_client::Hub;
use ensemble_core::protocol::*;

/// Subscription pattern covering everything the game publishes: combat events
/// under `/subit/game/event/*` and telemetry under `/subit/game/telemetry/*`.
const GAME_EVENTS_PATTERN: &str = "/subit/game/**";

fn main() {
    tracing_subscriber::fmt::init();

    // The window (feature `ui`, run with `--ui`) owns the main thread —
    // winit's event loop refuses to start anywhere else. The conductor
    // runs on its own thread beside it; closing the window ends the run.
    #[cfg(feature = "ui")]
    if std::env::args().any(|arg| arg == "--ui") {
        ui_main();
        return;
    }

    let (monitor, _monitor_view) = audio_voice::monitor::channel();
    block_on_conductor(monitor);
}

/// The `--ui` path: the conductor runs on its own thread while the
/// window owns the main thread.
#[cfg(feature = "ui")]
fn ui_main() {
    let (monitor, monitor_view) = audio_voice::monitor::channel();
    let conductor_thread = std::thread::Builder::new()
        .name("audio-voice-conductor".into())
        .spawn(move || block_on_conductor(monitor))
        .expect("the conductor thread spawns");
    audio_voice::monitor::app::run(monitor_view);
    // The window closed — the run is over. Don't join: the conductor
    // loops forever, and an abrupt stop matches a Ctrl+C (the hub cleans
    // a dropped connection up).
    drop(conductor_thread);
}

/// Run the conductor to completion on a fresh multi-thread runtime.
fn block_on_conductor(monitor: Monitor) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the tokio runtime builds")
        .block_on(run_conductor(monitor));
}

async fn run_conductor(monitor: Monitor) {
    let mut hub = connect_with_retry().await;
    tracing::info!(voice_id = hub.voice_id, "connected to Ensemble hub");

    hub.subscribe(GAME_EVENTS_PATTERN)
        .await
        .expect("hub rejected our event subscription");

    let quantiser = Quantiser::new(STUB_BPM, Subdivision::Sixteenth);
    let mut score = MusicState::default();
    // The first chord lands on the next 16th.
    let mut slot_start = quantiser.next_boundary(hub.now().await);
    let mut slot_end = slot_start;
    // The chord sounding right now: bass and pad notes a cut must silence.
    let mut sounding: Vec<(u8, u8)> = Vec::new();
    let mut aggro_locks: u32 = 0;
    let mut last_smooth_tick = 0.0f64;
    // The sustained voices' sounding tones — the common-tone legato's
    // memory (held tones skip the re-play; the rest get their note-offs).
    let mut sustained: Vec<(u8, u8)> = Vec::new();

    // The form: the piece's tonal geography — home, sequences, episodes —
    // ticked when a phrase completes. Time-seeded so every run tells a
    // different musical story.
    let mut form = Form::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos() as u64)
            .unwrap_or(0),
    );
    let mut form_rng = SmallRng::seed_from_u64(rand::random());

    // The composer: a slow planning tier over the engine. Accepted plans
    // arrive on the watch channel and steer the harmony until they deplete;
    // every failure keeps the current phrase, and a depleted one falls back
    // to the deterministic cycle. Holding our twin of the watch sender keeps
    // the receiver's `changed()` from erroring when the model is disabled.
    let composer_enabled = audio_voice::composer::enabled_from_env();
    let (plan_tx, mut plan_rx) = tokio::sync::watch::channel(None);
    let _plan_tx = plan_tx.clone();
    let (plan_requests, plan_requests_rx) =
        tokio::sync::mpsc::channel::<PlanRequest>(8);
    if composer_enabled {
        audio_voice::composer::spawn(
            audio_voice::composer::OllamaClient::from_env(),
            plan_requests_rx,
            plan_tx,
        );
        tracing::info!("the composer is listening (ollama)");
    } else {
        tracing::info!("the composer is disabled (OLLAMA_DISABLED=1)");
        drop(plan_requests_rx); // requests close harmlessly
    }
    // The monitor: the conductor publishes a snapshot wherever the state
    // changes; the optional window renders the latest one. Nothing
    // queues — an absent window costs nothing.
    let mut conductor = ConductorMonitor {
        monitor,
        composer_enabled,
        last_replan: None,
        last_tallies: None,
    };
    let mut last_intent: Option<String> = None;
    let mut event_log: VecDeque<EventLogEntry> = VecDeque::new();

    let mut history: VecDeque<SlotRecord> = VecDeque::new();
    let mut tallies = Tally::default();
    let mut last_curve: Option<Curve> = None;
    // Set when an aggio spike cut the slot short; consumed by the next
    // rendered slot's history record.
    let mut pending_cut = false;

    loop {
        // Hub-reported errors (e.g. rejected subscriptions) are non-fatal by
        // protocol design; log and carry on.
        while let Some(err) = hub.try_recv_error() {
            tracing::warn!(code = %err.code, "hub error: {}", err.message);
        }

        // Wait on the next game event or the next change point — whichever
        // comes first keeps the score flowing while staying reactive to
        // combat.
        let wait = Duration::from_secs_f64((slot_end - hub.now().await).max(0.0));
        tokio::select! {
            msg = hub.recv_action() => {
                let Some(msg) = msg else {
                    tracing::error!("hub connection closed");
                    break;
                };

                let Value::Map(routed) = &msg.payload else {
                    continue;
                };
                let Some(address) = get_string(routed, "address") else {
                    continue;
                };
                let Value::Map(payload) =
                    get_value(routed, "payload").unwrap_or(Value::Null)
                else {
                    tracing::warn!(address = %address, "unrecognised event payload");
                    continue;
                };

                match address.as_str() {
                    "/subit/game/telemetry/world" => {
                        tracing::info!(
                            integrity = get_float(&payload, "integrity").unwrap_or(0.0),
                            "world integrity reported"
                        );
                    }
                    "/subit/game/telemetry/objective" => {
                        // The objective's slow field: the run's arc — the
                        // composer reads it when it next plans, and the
                        // arrangement colours the texture with it now.
                        if let Some(progress) = get_float(&payload, "progress") {
                            score.objective = (progress as f32).clamp(0.0, 1.0);
                        }
                    }
                    "/subit/game/telemetry/aggro" => {
                        if let Some(locks) = parse_aggro(&payload)
                            && locks != aggro_locks
                        {
                            tracing::info!(locks, "aggro locks updated");
                            aggro_locks = locks;
                            tallies.record_aggro(locks);

                            // Mid-slot recalculation: tick the smoother to
                            // now, then move the change point if the new
                            // harmonic rhythm calls for it.
                            let now = hub.now().await;
                            let dt = (now - last_smooth_tick).max(0.0) as f32;
                            last_smooth_tick = now;
                            retune(&mut score, locks, dt);

                            let grid = if music::is_additive(score.rhythm_intensity) {
                                QUAVER_SECS
                            } else {
                                CROTCHET_SECS
                            };
                            let new_period = current_slot_period_secs(score.rhythm_intensity);
                            let cut = change_point(slot_start, slot_end, new_period, grid, now);
                            if cut < slot_end {
                                // Cut short: silence the sounding chord at
                                // the new boundary; the next slot renders
                                // from there.
                                for &(channel, note) in &sounding {
                                    let msg = midi_note_off(channel, note, cut);
                                    if let Err(err) = hub.send_action(msg).await {
                                        tracing::error!("failed to send note-off: {err:?}");
                                    }
                                }
                                tracing::info!(when = cut, "harmonic rhythm cut the chord short");
                                slot_end = cut;
                                pending_cut = true;
                            }

                            // The smoother tick and the (possibly moved)
                            // change point are visible — publish them.
                            conductor.publish(
                                now,
                                slot_start,
                                slot_end,
                                PublishState {
                                    score: &score,
                                    form: &form,
                                    intent: last_intent.clone(),
                                    history: history.make_contiguous(),
                                    event_log: event_log.make_contiguous(),
                                },
                            );
                        }
                    }
                    _ => {
                        let Some(event) = parse_event(&payload) else {
                            tracing::warn!(address = %address, "unrecognised event payload");
                            continue;
                        };
                        // The composer reads the dramatic moments — sweeps,
                        // level-ups, corruption boundaries, descents.
                        tallies.record_event(&event);

                        // The corruption boundary is the harmony's: while
                        // the player stands in degraded data, every
                        // chromatic mapping mutates.
                        match event {
                            GameEvent::CorruptionEnter if !score.degraded => {
                                score.degraded = true;
                                tracing::info!("the harmony degrades in the corrupted zone");
                            }
                            GameEvent::CorruptionExit if score.degraded => {
                                score.degraded = false;
                                tracing::info!("the harmony restores");
                            }
                            _ => {}
                        }

                        let now = hub.now().await;
                        let note = perform_event(event);
                        let when = quantiser.next_boundary(now);
                        event_log.push_back(EventLogEntry { when: now, event });
                        while event_log.len() > EVENT_LOG_LEN {
                            event_log.pop_front();
                        }
                        conductor.publish(
                            now,
                            slot_start,
                            slot_end,
                            PublishState {
                                score: &score,
                                form: &form,
                                intent: last_intent.clone(),
                                history: history.make_contiguous(),
                                event_log: event_log.make_contiguous(),
                            },
                        );
                        match hub.send_action(midi_play(&note, when)).await {
                            Ok(()) => tracing::info!(
                                address = %address,
                                when,
                                note = note.note as i64,
                                "performed event"
                            ),
                            Err(err) => {
                                tracing::error!(address = %address, "failed to schedule note: {err:?}")
                            }
                        }
                    }
                }
            }
            _ = tokio::time::sleep(wait) => {
                // The change point: render the next chord slot.
                let chord_vocab = score.harmony.current_vocab(score.slot);
                let held = score.rhythm_hold > 0.0;
                let (notes, end) = Engine::slot_notes(&mut score, slot_end);

                // The common-tone legato: a sustained voice's tone still
                // sounding from the last slot is HELD (not re-played — the
                // bridge drops a re-play while a key is down), and the
                // tones that don't persist get their note-offs at the
                // boundary. The bass pulses and patterns stay percussive.
                let next_sustained: Vec<(u8, u8)> = notes
                    .iter()
                    .filter(|(_, n)| n.channel == CHANNEL_DISTANT || n.channel == CHANNEL_AIRY)
                    .map(|(_, n)| (n.channel, n.note))
                    .collect();
                let (held_tones, off_tones) =
                    music::sustain_split(&sustained, &next_sustained);
                for &(channel, note) in &off_tones {
                    if let Err(err) = hub.send_action(midi_note_off(channel, note, slot_end)).await
                    {
                        tracing::error!("failed to send legato note-off: {err:?}");
                    }
                }
                // The ties live: every held tone's stop is re-armed from
                // the boundary — the original play's scheduled off can
                // never outlive the legato (it would silence a pad the
                // daemon still believes is ringing), and long pivots
                // survive late chord changes.
                let window = tie_window(end - slot_end);
                for &(channel, note) in &held_tones {
                    let msg = midi_cancel(channel, note, slot_end, window);
                    if let Err(err) = hub.send_action(msg).await {
                        tracing::error!("failed to re-arm a tie: {err:?}");
                    }
                }
                sustained = next_sustained;

                // The cut's silencing set: every score-channel note.
                sounding = notes
                    .iter()
                    .filter(|(_, n)| n.channel <= CHANNEL_PATTERN_B)
                    .map(|(_, n)| (n.channel, n.note))
                    .collect();
                for (at, note) in notes {
                    if held_tones.contains(&(note.channel, note.note)) {
                        continue; // the common tone keeps ringing
                    }
                    let when = slot_end + at;
                    if let Err(err) = hub.send_action(midi_play(&note, when)).await {
                        tracing::error!("failed to schedule score note: {err:?}");
                    }
                }
                slot_start = slot_end;
                slot_end = end;

                // The bass channel's mod wheel is the phrase's own: the
                // dynamic arc shapes the filter across the slot, lifted
                // into the next phrase when the arc ends settled — one
                // point per quaver, on the bass pulse grid.
                for (at, wheel) in music::wheel_trajectory(&score, slot_start, slot_end) {
                    let msg = midi_cc(CHANNEL_BASS, MOD_WHEEL_CC, wheel, slot_start + at);
                    if let Err(err) = hub.send_action(msg).await {
                        tracing::error!("failed to schedule the bass mod wheel: {err:?}");
                    }
                }

                // The pads breathe: the sustained channels' expression
                // follows the phrase arc through the general dynamic.
                score.episode = matches!(form, Form::Episode { .. });
                let level = music::expression_level(&score);
                for channel in [CHANNEL_DISTANT, CHANNEL_AIRY] {
                    let msg = midi_cc(channel, EXPRESSION_CC, level, 0.0);
                    if let Err(err) = hub.send_action(msg).await {
                        tracing::error!("failed to send expression: {err:?}");
                    }
                }

                // The composer's memory: the slot joins the history, then
                // the drained tallies and the plan's remaining slots decide
                // whether to ask for the next phrase.
                history.push_back(SlotRecord {
                    chord: chord_vocab as u32,
                    duration_secs: (end - slot_start) as f32,
                    cut: pending_cut,
                    held,
                });
                pending_cut = false;
                while history.len() > audio_voice::composer::HISTORY_LEN {
                    history.pop_front();
                }

                // A phrase completed — a plan's last slot, or a full turn
                // of the fallback cycle — and the form ticks: the next
                // phrase lives wherever the form moves to.
                let phrase_completed = match score.harmony.slots_remaining() {
                    Some(remaining) => remaining == 0,
                    None => music::cycle_turn_complete(score.slot),
                };
                if phrase_completed {
                    if form.tick(&mut form_rng) {
                        let summary = form.summary();
                        tracing::info!(key = %summary.key, state = summary.state, "the form moved");
                    }
                    score.key.tonic = form.key_offset();
                    // The cycle's next tiles: rolled from the form's RNG
                    // and stitched from whatever sounded last.
                    advance_cycle_tiles(&mut score, &mut form_rng);
                }

                let drained = tallies.drain();
                conductor.note_tallies(drained.clone());
                if let Some(reason) = audio_voice::composer::should_replan(
                    score.harmony.slots_remaining(),
                    &drained,
                ) {
                    conductor.note_replan(reason);
                    let request = PlanRequest {
                        context: audio_voice::composer::build_context(&ScoreContext {
                            history: history.iter().copied().collect(),
                            tallies: drained.clone(),
                            sounding_chord: Some(chord_vocab),
                            degraded: score.degraded,
                            progress: score.objective,
                            form: form.summary(),
                        }),
                        prev_chord: Some(chord_vocab),
                        previous_curve: last_curve.clone(),
                    };
                    match plan_requests.try_send(request) {
                        Ok(()) => tracing::info!(?reason, "asked the composer for the next phrase"),
                        Err(err) => tracing::debug!(?reason, "composer busy: {err}"),
                    }
                }

                // The slot's record, cut state and any replan decision are
                // in — publish them.
                conductor.publish(
                    hub.now().await,
                    slot_start,
                    slot_end,
                    PublishState {
                        score: &score,
                        form: &form,
                        intent: last_intent.clone(),
                        history: history.make_contiguous(),
                        event_log: event_log.make_contiguous(),
                    },
                );
            }
            plan_changed = plan_rx.changed() => {
                // The composer published a plan — install it for the next
                // slot boundary. The sender can't close while the daemon
                // holds its twin, but an error shouldn't spin the loop.
                match plan_changed {
                    Ok(()) => {
                        let outcome = plan_rx.borrow_and_update().clone();
                        if let Some(outcome) = outcome {
                            last_intent = Some(outcome.plan.intent.clone());
                            tracing::info!(
                                slots = outcome.plan.slots.len(),
                                intent = %outcome.plan.intent,
                                "the composer's plan is live"
                            );
                            last_curve = Some(outcome.curve.clone());
                            // The plan's dynamic arc: its own shape, or
                            // the gentle default when it drew none.
                            let dynamic = if outcome.plan.curve_dynamic.len() >= 2 {
                                Curve {
                                    upper: outcome.plan.curve_dynamic.clone(),
                                    lower: outcome.plan.curve_dynamic.clone(),
                                }
                            } else {
                                music::default_phrase_curve()
                            };
                            score.harmony = HarmonySource::Plan {
                                // The plan's slots carry their inversions.
                                slots: outcome.plan.slots.clone(),
                                curve: outcome.curve,
                                dynamic,
                                cursor: 0,
                            };
                            // The pads breathe the new phrase's opening
                            // level straight away.
                            score.episode = matches!(form, Form::Episode { .. });
                            let level = music::expression_level(&score);
                            for channel in [CHANNEL_DISTANT, CHANNEL_AIRY] {
                                let msg = midi_cc(channel, EXPRESSION_CC, level, 0.0);
                                if let Err(err) = hub.send_action(msg).await {
                                    tracing::error!("failed to send expression: {err:?}");
                                }
                            }

                            // The plan's install is visible — publish it.
                            conductor.publish(
                                hub.now().await,
                                slot_start,
                                slot_end,
                                PublishState {
                                    score: &score,
                                    form: &form,
                                    intent: last_intent.clone(),
                                    history: history.make_contiguous(),
                                    event_log: event_log.make_contiguous(),
                                },
                            );
                        }
                    }
                    Err(_) => tracing::warn!("the composer's watch channel closed"),
                }
            }
        }
    }
}

/// The conductor's side of the monitor: the publisher plus the composer
/// trace the snapshots carry.
struct ConductorMonitor {
    monitor: Monitor,
    composer_enabled: bool,
    last_replan: Option<ReplanReason>,
    last_tallies: Option<EventTallies>,
}

/// The pieces the snapshot builder reads from the conductor's locals.
struct PublishState<'a> {
    score: &'a MusicState,
    form: &'a Form,
    intent: Option<String>,
    history: &'a [SlotRecord],
    event_log: &'a [EventLogEntry],
}

impl ConductorMonitor {
    /// Record why the composer was asked for the next phrase.
    fn note_replan(&mut self, reason: ReplanReason) {
        self.last_replan = Some(reason);
    }

    /// Record the tallies that accompanied the last replan decision.
    fn note_tallies(&mut self, tallies: EventTallies) {
        self.last_tallies = Some(tallies);
    }

    /// Publish a snapshot of the conductor's state — the newest wins,
    /// nothing queues.
    fn publish(&self, now: f64, slot_start: f64, slot_end: f64, state: PublishState<'_>) {
        let form_summary = state.form.summary();
        self.monitor.publish(UiSnapshot::capture(SnapshotInputs {
            hub_time: now,
            slot_start,
            slot_end,
            state: state.score,
            form: &form_summary,
            intent: state.intent,
            history: state.history,
            events: state.event_log,
            composer: audio_voice::snapshot::ComposerInputs {
                enabled: self.composer_enabled,
                last_replan: self.last_replan,
                last_tallies: self.last_tallies.clone(),
            },
        }));
    }
}

async fn connect_with_retry() -> Hub {
    loop {
        match Hub::connect_with_discovery("subit-audio-voice").await {
            Ok(hub) => return hub,
            Err(err) => {
                tracing::warn!("Ensemble hub unreachable ({err:?}); retrying in 2s");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

/// Recover the game event from a routed action's inner payload.
fn parse_event(payload: &BTreeMap<String, Value>) -> Option<GameEvent> {
    match get_string(payload, "type").as_deref()? {
        "attack_primary" => Some(GameEvent::AttackPrimary),
        "mob_sweep" => Some(GameEvent::MobSweep {
            kill_count: get_integer(payload, "kill_count").unwrap_or(0).max(0) as u32,
            combo: get_integer(payload, "combo").unwrap_or(1).max(1) as u32,
        }),
        "projectile_throw" => Some(GameEvent::ProjectileThrow),
        "shield_reflect" => Some(GameEvent::ShieldReflect),
        "level_up" => Some(GameEvent::LevelUp),
        "dash" => Some(GameEvent::Dash),
        "grenade_blast" => Some(GameEvent::GrenadeBlast),
        "nova" => Some(GameEvent::Nova),
        "cloak" => Some(GameEvent::Cloak),
        "corruption_enter" => Some(GameEvent::CorruptionEnter),
        "corruption_exit" => Some(GameEvent::CorruptionExit),
        "descent" => Some(GameEvent::Descent),
        "death" => Some(GameEvent::Death),
        "drone_shot" => Some(GameEvent::DroneShot),
        "drone_down" => Some(GameEvent::DroneDown),
        "drone_online" => Some(GameEvent::DroneOnline),
        "vault_open" => Some(GameEvent::VaultOpen),
        _ => None,
    }
}

/// Recover the aggro-lock count from an aggro telemetry payload.
fn parse_aggro(payload: &BTreeMap<String, Value>) -> Option<u32> {
    get_integer(payload, "locks").map(|locks| locks.max(0) as u32)
}

/// Build a scheduled `/midi/play` action for the MIDI bridge.
fn midi_play(note: &NoteEvent, when: f64) -> WireMessage {
    action(
        "/midi/play",
        SignalType::Event,
        when,
        Value::Tuple(vec![
            Value::Integer(note.channel as i64),
            Value::Integer(note.note as i64),
            Value::Integer(note.velocity as i64),
            Value::Float(FloatValue::new(note.duration_secs)),
        ]),
    )
}

/// Build a (possibly timestamped) `/midi/note-off` action — the early cut of
/// a note that would otherwise ring to its scheduled off.
fn midi_note_off(channel: u8, note: u8, when: f64) -> WireMessage {
    action(
        "/midi/note-off",
        SignalType::Event,
        when,
        Value::Tuple(vec![Value::Integer(channel as i64), Value::Integer(note as i64)]),
    )
}

/// Build a (possibly timestamped) `/midi/cancel` action — a held tone's
/// stop re-armed: the bridge drops the pending note-off and arms a fresh
/// one `window_secs` out.
fn midi_cancel(channel: u8, note: u8, when: f64, window_secs: f64) -> WireMessage {
    action(
        "/midi/cancel",
        SignalType::Event,
        when,
        Value::Tuple(vec![
            Value::Integer(channel as i64),
            Value::Integer(note as i64),
            Value::Float(FloatValue::new(window_secs)),
        ]),
    )
}

/// Build a (possibly timestamped) `/midi/cc` action for the MIDI bridge.
fn midi_cc(channel: u8, controller: u8, value: u8, when: f64) -> WireMessage {
    action(
        "/midi/cc",
        SignalType::Event,
        when,
        Value::Tuple(vec![
            Value::Integer(channel as i64),
            Value::Integer(controller as i64),
            Value::Integer(value as i64),
        ]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mob_sweep_with_combo() {
        let mut payload = BTreeMap::new();
        payload.insert("type".into(), Value::String("mob_sweep".into()));
        payload.insert("kill_count".into(), Value::Integer(3));
        payload.insert("combo".into(), Value::Integer(5));
        let event = parse_event(&payload).expect("mob_sweep parses");
        assert_eq!(
            event,
            GameEvent::MobSweep {
                kill_count: 3,
                combo: 5
            }
        );
    }

    #[test]
    fn parses_aggro_locks() {
        let mut payload = BTreeMap::new();
        payload.insert("type".into(), Value::String("aggro_locks".into()));
        payload.insert("locks".into(), Value::Integer(3));
        assert_eq!(parse_aggro(&payload), Some(3));
        payload.insert("locks".into(), Value::Integer(-2));
        assert_eq!(parse_aggro(&payload), Some(0));
        payload.remove("locks");
        assert_eq!(parse_aggro(&payload), None);
    }

    #[test]
    fn mob_sweep_defaults_combo_to_one() {
        let mut payload = BTreeMap::new();
        payload.insert("type".into(), Value::String("mob_sweep".into()));
        payload.insert("kill_count".into(), Value::Integer(2));
        let event = parse_event(&payload).expect("mob_sweep parses");
        assert_eq!(
            event,
            GameEvent::MobSweep {
                kill_count: 2,
                combo: 1
            }
        );
    }

    #[test]
    fn cancel_actions_carry_a_channel_note_and_window() {
        let msg = midi_cancel(1, 62, 4.0, 3.5);
        let Value::Map(map) = &msg.payload else {
            panic!("expected map payload");
        };
        assert_eq!(get_string(map, "address"), Some("/midi/cancel".into()));
        let Value::Tuple(values) = get_value(map, "payload").unwrap() else {
            panic!("expected tuple payload");
        };
        assert_eq!(
            values,
            vec![
                Value::Integer(1),
                Value::Integer(62),
                Value::Float(FloatValue::new(3.5)),
            ]
        );
    }

    #[test]
    fn cc_actions_carry_a_three_byte_tuple() {
        let msg = midi_cc(0, MOD_WHEEL_CC, 96, 0.0);
        let Value::Map(map) = &msg.payload else {
            panic!("expected map payload");
        };
        assert_eq!(get_string(map, "address"), Some("/midi/cc".into()));
        let Value::Tuple(values) = get_value(map, "payload").unwrap() else {
            panic!("expected tuple payload");
        };
        assert_eq!(
            values,
            vec![
                Value::Integer(0),
                Value::Integer(MOD_WHEEL_CC as i64),
                Value::Integer(96),
            ]
        );
    }

    #[test]
    fn note_off_actions_carry_a_two_byte_tuple() {
        let msg = midi_note_off(0, 38, 3.5);
        let Value::Map(map) = &msg.payload else {
            panic!("expected map payload");
        };
        assert_eq!(get_string(map, "address"), Some("/midi/note-off".into()));
        let Value::Tuple(values) = get_value(map, "payload").unwrap() else {
            panic!("expected tuple payload");
        };
        assert_eq!(values, vec![Value::Integer(0), Value::Integer(38)]);
    }
}
