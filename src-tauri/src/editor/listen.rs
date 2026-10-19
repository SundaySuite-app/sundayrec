//! The sound step's listening, on the playback engine (B5b).
//!
//! «Før/Etter» used to render a 20-second mp3 through ffmpeg for every choice (the wait the
//! step's «Gjengir …» text was about). Now the engine ([`crate::playback`]) plays the
//! recording itself, on the sound card, with the cuts: the programme as it is (Før) and the
//! programme through the chain (Etter) are both in its ring, and the choice between them is a
//! switch the audio callback crossfades in 20 ms. No file is written.
//!
//! One session at a time, kept in [`ListenEngine`] (managed Tauri state):
//!
//! * [`open`] — the recording and its cuts, paused, «Før» chosen, with an EMPTY chain. It does
//!   not wait for anything: the programme measure «Etter» needs can take a minute the first
//!   time, and «Før» does not need it.
//! * [`set_chain`] — the export's chain for a choice (profile or mixer, channel repair, «Tale»'s
//!   decision, and the programme's level plan) from [`super::listen_plan`], the same function
//!   the preview and the export use, installed in the running engine. A newer ask wins over an
//!   older one that finishes later. The engine crossfades to it; the ear gets it after at most
//!   half a second of what is already buffered.
//! * [`transport`] — play, pause, jump (source seconds, as a click on the waveform would give),
//!   and the Før/Etter switch.
//! * [`close`] — stop and let go of the device.
//!
//! What the engine tells (position, end, underrun, xrun, error) goes to the window as
//! `editor://listen`, ~30 times a second for the position.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::playback::{PlaybackEvent, Player};

/// What `editor_listen_open` answers.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorListenOpened.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorListenOpened {
    /// Length of the programme (the recording minus its cuts), seconds.
    pub program_secs: f64,
}

/// What the engine tells the window (`editor://listen`).
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[ts(export, export_to = "EditorListenEvent.ts")]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EditorListenEvent {
    /// The device has been handed audio up to here. `gen` rises with every jump: a position of
    /// an older generation than one already seen is stale.
    Position {
        gen: u32,
        #[serde(rename = "programSecs")]
        program_secs: f64,
        #[serde(rename = "sourceSecs")]
        source_secs: f64,
    },
    /// The programme played to its end.
    Ended { gen: u32 },
    /// The ring ran dry while playing: a gap you can hear.
    Underrun { total: u32 },
    /// The sound card reported a glitch.
    Xrun { total: u32 },
    /// The device or the file failed; the listening has stopped.
    Error { message: String },
}

impl From<PlaybackEvent> for EditorListenEvent {
    fn from(e: PlaybackEvent) -> Self {
        match e {
            PlaybackEvent::Position {
                gen,
                program_secs,
                source_secs,
            } => Self::Position {
                gen: gen as u32,
                program_secs,
                source_secs,
            },
            PlaybackEvent::Ended { gen } => Self::Ended { gen: gen as u32 },
            PlaybackEvent::Underrun { total } => Self::Underrun {
                total: total as u32,
            },
            PlaybackEvent::Xrun { total } => Self::Xrun { total: total as u32 },
            PlaybackEvent::Error { message } => Self::Error { message },
        }
    }
}

/// What the sound step asks the transport to do.
#[derive(Debug, Clone, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorListenAction.ts")]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EditorListenAction {
    Play,
    Pause,
    /// Jump to this many seconds of SOURCE time (inside a cut: the next part that stays).
    SeekSource { secs: f64 },
    /// «Etter» (`true`, through the chain) or «Før» (`false`, the programme as it is).
    Side { after: bool },
}

struct Session {
    player: Player,
    /// Chain asks handed out, and the newest one installed: a slow ask that finishes after a
    /// newer one must not put its older chain over it.
    asked: AtomicU64,
    installed: u64,
}

/// A session that has been taken out of the engine and is yet to be stopped.
pub struct Detached(Session);

impl Detached {
    /// Fade out, stop every thread and release the sound card. Blocking.
    pub fn close(self) {
        self.0.player.close();
    }
}

/// The one listening session, if there is one.
#[derive(Default)]
pub struct ListenEngine {
    session: Mutex<Option<Session>>,
    /// Bumped by every open and close, so an ask made for a session that is gone is dropped.
    epoch: AtomicU64,
}

impl ListenEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Session>> {
        self.session.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a session is open (for tests and diagnosis).
    pub fn is_open(&self) -> bool {
        self.lock().is_some()
    }

    /// Take the session out, so no more asks reach it. [`Detached::close`] then stops it, which
    /// blocks (the engine fades out and joins its threads): do that off the async runtime.
    pub fn detach(&self) -> Option<Detached> {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.lock().take().map(Detached)
    }

    /// Run the transport action. No session is not an error: the step may be left as the
    /// click arrives.
    pub fn transport(&self, action: &EditorListenAction) {
        let guard = self.lock();
        let Some(s) = guard.as_ref() else {
            return;
        };
        match action {
            EditorListenAction::Play => s.player.play(),
            EditorListenAction::Pause => s.player.pause(),
            EditorListenAction::SeekSource { secs } => {
                if secs.is_finite() {
                    s.player.seek_source(*secs);
                }
            }
            EditorListenAction::Side { after } => s.player.set_wet(*after),
        }
    }
}

#[cfg(feature = "editor")]
pub use real::{open, set_chain};

#[cfg(not(feature = "editor"))]
pub async fn open(
    _engine: &ListenEngine,
    _emit: impl Fn(EditorListenEvent) + Send + Sync + 'static,
    _req: &super::EditorMasterPreviewRequest,
    _source: &str,
) -> AppResult<EditorListenOpened> {
    Err(AppError::Validation(
        "feature_disabled: editor.listen requires a build with `--features editor`".into(),
    ))
}

#[cfg(not(feature = "editor"))]
pub async fn set_chain(
    _engine: &ListenEngine,
    _export: &super::ExportEngine,
    _req: &super::EditorMasterPreviewRequest,
    _source: &str,
) -> AppResult<bool> {
    Ok(false)
}

#[cfg(feature = "editor")]
mod real {
    use super::*;
    use crate::editor::{listen_plan, load_recording, pcm, EditorMasterPreviewRequest};
    use crate::editor::{ExportEngine, ListenPlan};
    use crate::playback::source::FfmpegOpener;
    use crate::playback::Programme;
    use sundayrec_core::chain::Chain;
    use sundayrec_core::editor::{build_keeps, CutRegion};
    use sundayrec_core::playback::CutPlan;

    /// Open the session for `source` with the cuts of `req`: paused, at `req.start_sec` (source
    /// seconds), «Før» chosen, no chain yet. Replaces a session that is open.
    pub async fn open(
        engine: &ListenEngine,
        emit: impl Fn(EditorListenEvent) + Send + Sync + 'static,
        req: &EditorMasterPreviewRequest,
        source: &str,
    ) -> AppResult<EditorListenOpened> {
        if !std::path::Path::new(source).exists() {
            return Err(AppError::Validation("file_not_found".into()));
        }
        let info = load_recording(source).await?;
        if !info.has_audio {
            return Err(AppError::Validation("no_audio_remaining".into()));
        }
        let fmt = pcm::pcm_format(info.sample_rate, info.channels);
        let duration = req
            .full_length_sec
            .filter(|d| d.is_finite() && *d > 0.0)
            .unwrap_or(info.duration_sec);
        if !duration.is_finite() || duration <= 0.0 {
            return Err(AppError::Validation("invalid_duration".into()));
        }
        let cuts: Vec<CutRegion> = req
            .cut_regions
            .iter()
            .map(|c| CutRegion {
                start: c.start,
                end: c.end,
            })
            .collect();
        let keeps = build_keeps(&cuts, duration);
        if keeps.is_empty() {
            return Err(AppError::Validation("no_audio_remaining".into()));
        }
        let plan = CutPlan::new(&keeps, duration, fmt.sample_rate);
        let program_secs = plan.program_secs();
        let programme = Programme {
            plan,
            chain: Chain::default(),
            opener: FfmpegOpener {
                ffmpeg: crate::media::ffmpeg::ffmpeg_path().into(),
                input: source.into(),
                sample_rate: fmt.sample_rate,
                channels: fmt.channels,
            },
            source_channels: fmt.channels,
        };
        // The old session goes first: two streams on one device is a fight, and the old one's
        // ask for a chain must not land in the new.
        let old = engine.detach();
        let start = req.start_sec.max(0.0);
        let player = crate::util::off_runtime(move || {
            if let Some(s) = old {
                s.close();
            }
            let player = Player::open(
                programme,
                Box::new(move |e| emit(EditorListenEvent::from(e))),
            )?;
            // «Før» until a chain has been installed: with no chain «Etter» would be «Før».
            player.set_wet(false);
            if start > 0.0 {
                player.seek_source(start);
            }
            Ok::<_, String>(player)
        })
        .await?
        .map_err(|e| AppError::Recording(format!("listen: {e}")))?;
        engine.epoch.fetch_add(1, Ordering::AcqRel);
        *engine.lock() = Some(Session {
            player,
            asked: AtomicU64::new(0),
            installed: 0,
        });
        Ok(EditorListenOpened { program_secs })
    }

    /// Install the chain of `req` in the open session. Measures the programme first if that has
    /// not been done for this choice and these cuts (shared with the preview, the preparation
    /// and the export: [`listen_plan`]). `false` = nothing to install it in, or a newer ask
    /// came in while this one measured.
    pub async fn set_chain(
        engine: &ListenEngine,
        export: &ExportEngine,
        req: &EditorMasterPreviewRequest,
        source: &str,
    ) -> AppResult<bool> {
        let (ticket, epoch) = {
            let g = engine.lock();
            let Some(s) = g.as_ref() else {
                return Ok(false);
            };
            (
                s.asked.fetch_add(1, Ordering::AcqRel) + 1,
                engine.epoch.load(Ordering::Acquire),
            )
        };
        let ListenPlan { chain, plan, .. } = listen_plan(export, req, source).await?;
        // The export's order: the chain, then the loudness plan's gain and limiter.
        let chain = chain.with_gain_plan(&plan);
        let mut g = engine.lock();
        let Some(s) = g.as_mut() else {
            return Ok(false);
        };
        if engine.epoch.load(Ordering::Acquire) != epoch || ticket <= s.installed {
            return Ok(false);
        }
        s.installed = ticket;
        s.player.set_chain(chain);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_events_reach_the_window_in_their_own_words() {
        let e = EditorListenEvent::from(PlaybackEvent::Position {
            gen: 3,
            program_secs: 1.5,
            source_secs: 61.5,
        });
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({
                "kind": "position", "gen": 3, "programSecs": 1.5, "sourceSecs": 61.5
            })
        );
        assert_eq!(
            serde_json::to_value(EditorListenEvent::from(PlaybackEvent::Underrun { total: 2 }))
                .unwrap(),
            serde_json::json!({ "kind": "underrun", "total": 2 })
        );
    }

    #[test]
    fn the_transport_takes_its_actions_as_the_window_writes_them() {
        let a: EditorListenAction =
            serde_json::from_str(r#"{"kind":"seekSource","secs":12.5}"#).unwrap();
        assert_eq!(a, EditorListenAction::SeekSource { secs: 12.5 });
        let a: EditorListenAction = serde_json::from_str(r#"{"kind":"side","after":true}"#).unwrap();
        assert_eq!(a, EditorListenAction::Side { after: true });
        assert!(serde_json::from_str::<EditorListenAction>(r#"{"kind":"burn"}"#).is_err());
    }

    /// The step may be left as the click arrives: nothing open is neither an error nor a panic.
    #[test]
    fn a_transport_action_with_nothing_open_does_nothing() {
        let engine = ListenEngine::new();
        engine.transport(&EditorListenAction::Play);
        engine.transport(&EditorListenAction::Side { after: false });
        engine.transport(&EditorListenAction::SeekSource { secs: f64::NAN });
        assert!(!engine.is_open());
        assert!(engine.detach().is_none());
    }
}
