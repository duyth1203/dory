//! A player with no sound whose clock moves only when a test says so.

use std::sync::{Arc, Mutex, PoisonError};

use crate::domain::Ms;
use crate::ports::{PlayState, Playback, PlaybackError, PlaybackSource, Player, PlayerStatus};

type Shared = Arc<Mutex<State>>;

#[derive(Debug)]
struct State {
    duration: Ms,
    state: PlayState,
    position: Ms,
}

fn lock(state: &Shared) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
pub struct FakePlayback {
    opened: Mutex<Vec<(PlaybackSource, Shared)>>,
    fail: Mutex<Option<String>>,
}

impl FakePlayback {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes the next `open` fail with `why`.
    pub fn fail_next_open(&self, why: &str) {
        *self.fail.lock().unwrap_or_else(PoisonError::into_inner) = Some(why.to_owned());
    }

    /// Every source opened so far, oldest first.
    pub fn opened(&self) -> Vec<PlaybackSource> {
        self.opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(source, _)| source.clone())
            .collect()
    }

    /// Lets `by` of audio play on every player that is playing.
    pub fn advance(&self, by: Ms) {
        for (_, state) in self
            .opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            let mut state = lock(state);
            if state.state == PlayState::Playing {
                state.position = Ms(state.position.0.saturating_add(by.0).min(state.duration.0));
                if state.position == state.duration {
                    state.state = PlayState::Ended;
                }
            }
        }
    }
}

impl Playback for FakePlayback {
    fn open(&self, source: &PlaybackSource) -> Result<Box<dyn Player>, PlaybackError> {
        if let Some(why) = self
            .fail
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            return Err(PlaybackError::Open(why));
        }
        let state = Arc::new(Mutex::new(State {
            duration: source.duration,
            state: PlayState::Paused,
            position: Ms(0),
        }));
        self.opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((source.clone(), Arc::clone(&state)));
        Ok(Box::new(FakePlayer { state }))
    }
}

struct FakePlayer {
    state: Shared,
}

impl Player for FakePlayer {
    fn play(&self) -> Result<(), PlaybackError> {
        let mut state = lock(&self.state);
        if state.state == PlayState::Ended {
            state.position = Ms(0);
        }
        state.state = PlayState::Playing;
        Ok(())
    }

    fn pause(&self) {
        let mut state = lock(&self.state);
        if state.state == PlayState::Playing {
            state.state = PlayState::Paused;
        }
    }

    fn seek(&self, to: Ms) -> Result<(), PlaybackError> {
        let mut state = lock(&self.state);
        state.position = Ms(to.0.min(state.duration.0));
        if state.position == state.duration && state.state == PlayState::Playing {
            state.state = PlayState::Ended;
        }
        Ok(())
    }

    fn status(&self) -> PlayerStatus {
        let state = lock(&self.state);
        PlayerStatus {
            state: state.state,
            position: state.position,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::ChannelLayout;

    fn source() -> PlaybackSource {
        PlaybackSource {
            path: PathBuf::from("a.flac"),
            layout: ChannelLayout::MicSystem,
            duration: Ms(1_000),
        }
    }

    #[test]
    fn plays_seeks_pauses_and_ends() {
        let playback = FakePlayback::new();
        let player = playback.open(&source()).unwrap();
        assert_eq!(player.status().state, PlayState::Paused);
        player.seek(Ms(400)).unwrap();
        player.play().unwrap();
        playback.advance(Ms(300));
        assert_eq!(player.status().position, Ms(700));
        player.pause();
        playback.advance(Ms(300));
        assert_eq!(player.status().position, Ms(700));
        player.play().unwrap();
        playback.advance(Ms(900));
        assert_eq!(
            player.status(),
            PlayerStatus {
                state: PlayState::Ended,
                position: Ms(1_000)
            }
        );
        player.play().unwrap();
        assert_eq!(player.status().position, Ms(0));
    }
}
