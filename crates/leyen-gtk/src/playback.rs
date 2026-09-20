//! Launching and stopping games from the window, one request per game at a time.

use std::cell::RefCell;
use std::collections::HashSet;

use leyen_model::i18n::gettext;
use leyen_model::models::Game;

use crate::daemon;

thread_local! {
    /// Games with a launch or stop in flight, by Leyen ID, so a double click or a
    /// click in both the library and Running Games sends one request.
    static IN_FLIGHT: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Releases the game on drop, so early returns and cancellation cannot keep it busy.
struct InFlight(String);

impl InFlight {
    fn claim(leyen_id: &str) -> Option<Self> {
        IN_FLIGHT
            .with(|set| set.borrow_mut().insert(leyen_id.to_string()))
            .then(|| Self(leyen_id.to_string()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        IN_FLIGHT.with(|set| set.borrow_mut().remove(&self.0));
    }
}

/// Stops the game when it runs and launches it otherwise. Returns what to tell the
/// user, or `None` when a request for the game is already on its way.
pub async fn launch_or_stop(game: &Game) -> Option<String> {
    let _in_flight = InFlight::claim(&game.leyen_id)?;
    let running = daemon::running_games_snapshot()
        .await
        .iter()
        .any(|snapshot| snapshot.game_id == game.id);

    Some(if running {
        match daemon::stop_game(&game.leyen_id).await {
            Ok(true) => gettext("Stopping {}…").replacen("{}", &game.title, 1),
            Ok(false) => gettext("Game is no longer running"),
            Err(reason) => reason,
        }
    } else {
        match daemon::launch_game(&game.leyen_id).await {
            Ok(()) => gettext("Launching {}…").replacen("{}", &game.title, 1),
            Err(reason) => reason,
        }
    })
}

/// Stops a running game. Returns a message only when something went wrong: the
/// game disappearing from Running Games says the rest.
pub async fn stop(leyen_id: &str) -> Option<String> {
    let _in_flight = InFlight::claim(leyen_id)?;
    match daemon::stop_game(leyen_id).await {
        Ok(true) => None,
        Ok(false) => Some(gettext("Game is no longer running")),
        Err(reason) => Some(reason),
    }
}
