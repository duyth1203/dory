use std::time::Duration;

use dory_core::domain::{Ms, RecordingId};
use dory_core::ports::{PlayState, Player, PlayerStatus};
use gpui_kit::{Context, SharedString, Task};

use super::core::Core;

const POLL: Duration = Duration::from_millis(100);

/// The selected recording's audio, shared by the player view and the transcript view.
///
/// One `Session` owns one `PlayerHandle`, so selecting another recording drops it and stops the
/// sound. The player view draws it and the transcript view seeks it ("Play from 12:24"); neither
/// holds a `Player` of its own, so there is one playhead. The audio opens on the first command,
/// not on selection, so arrowing through the sidebar opens no files. While playing it polls
/// the port every 100 ms and notifies only when the status changed; paused, it runs no timer.
pub struct PlayerHandle {
    core: Core,
    id: RecordingId,
    duration: Ms,
    player: Option<Box<dyn Player>>,
    status: PlayerStatus,
    unavailable: Option<SharedString>,
    poll: Option<Task<()>>,
}

impl PlayerHandle {
    pub(super) fn new(core: Core, id: RecordingId, duration: Ms) -> Self {
        Self {
            core,
            id,
            duration,
            player: None,
            status: PlayerStatus {
                state: PlayState::Paused,
                position: Ms(0),
            },
            unavailable: None,
            poll: None,
        }
    }

    pub fn recording(&self) -> &RecordingId {
        &self.id
    }

    pub fn position(&self) -> Ms {
        self.status.position
    }

    pub fn duration(&self) -> Ms {
        self.duration
    }

    pub fn is_playing(&self) -> bool {
        self.status.state == PlayState::Playing
    }

    /// Why the audio cannot play, for example while the take is still being saved.
    pub fn unavailable(&self) -> Option<&SharedString> {
        self.unavailable.as_ref()
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.is_playing() {
            if let Some(player) = &self.player {
                player.pause();
            }
            self.refresh(cx);
        } else {
            self.play(cx);
        }
    }

    /// Moves the playhead and keeps playing or staying paused.
    pub fn seek(&mut self, to: Ms, cx: &mut Context<Self>) {
        let to = Ms(to.0.min(self.duration.0));
        let Some(player) = self.open() else {
            cx.notify();
            return;
        };
        if let Err(error) = player.seek(to) {
            self.unavailable = Some(error.to_string().into());
        }
        self.refresh(cx);
    }

    /// Moves by `delta_ms`, clamped to the take: the slider's arrow and page keys.
    pub fn step(&mut self, delta_ms: i64, cx: &mut Context<Self>) {
        let to = (self.position().0 as i64 + delta_ms).clamp(0, self.duration.0 as i64);
        self.seek(Ms(to as u64), cx);
    }

    /// Seeks and plays: a transcript timestamp, or Return on a focused turn (UX-3).
    pub fn play_from(&mut self, to: Ms, cx: &mut Context<Self>) {
        self.seek(to, cx);
        if !self.is_playing() {
            self.play(cx);
        }
    }

    fn play(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self.open() else {
            cx.notify();
            return;
        };
        if let Err(error) = player.play() {
            self.unavailable = Some(error.to_string().into());
        }
        self.refresh(cx);
    }

    fn open(&mut self) -> Option<&dyn Player> {
        if self.player.is_none() {
            match self.core.dory().library().player(&self.id) {
                Ok(player) => {
                    self.player = Some(player);
                    self.unavailable = None;
                }
                Err(error) => self.unavailable = Some(error.to_string().into()),
            }
        }
        self.player.as_deref()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(player) = &self.player {
            self.status = player.status();
        }
        if self.is_playing() && self.poll.is_none() {
            self.poll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    let playing = this.update(cx, |this, cx| {
                        let Some(player) = &this.player else {
                            return false;
                        };
                        let status = player.status();
                        if status != this.status {
                            this.status = status;
                            cx.notify();
                        }
                        this.is_playing()
                    });
                    if !matches!(playing, Ok(true)) {
                        break;
                    }
                }
                let _ = this.update(cx, |this, _| this.poll = None);
            }));
        }
        cx.notify();
    }
}
