//! How durations, playtime and dates read in the library and the running games.

use leyen_model::i18n::gettext;

pub fn now_epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn playtime(playtime_seconds: u64) -> String {
    let hours = playtime_seconds / 3600;
    let minutes = (playtime_seconds % 3600) / 60;

    if hours > 0 {
        gettext("Playtime: {}h {}m")
            .replacen("{}", &hours.to_string(), 1)
            .replacen("{}", &minutes.to_string(), 1)
    } else if minutes > 0 {
        gettext("Playtime: {}m").replacen("{}", &minutes.to_string(), 1)
    } else {
        gettext("Playtime: {}s").replacen("{}", &playtime_seconds.to_string(), 1)
    }
}

pub fn duration_brief(total_seconds: u64) -> String {
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        gettext("{}h {}m")
            .replacen("{}", &hours.to_string(), 1)
            .replacen("{}", &minutes.to_string(), 1)
    } else if minutes > 0 {
        gettext("{}m {}s")
            .replacen("{}", &minutes.to_string(), 1)
            .replacen("{}", &seconds.to_string(), 1)
    } else {
        gettext("{}s").replacen("{}", &seconds.to_string(), 1)
    }
}

/// "Running for …", counted from `started_at` to now.
pub fn running_for(started_at_epoch_seconds: u64) -> String {
    let elapsed = now_epoch_seconds().saturating_sub(started_at_epoch_seconds);
    gettext("Running for {}").replacen("{}", &duration_brief(elapsed), 1)
}

pub fn last_played(epoch_seconds: u64) -> String {
    if epoch_seconds == 0 {
        return gettext("Last played: never");
    }

    let delta = now_epoch_seconds().saturating_sub(epoch_seconds);
    let ago = if delta < 60 {
        gettext("{}s ago").replacen("{}", &delta.to_string(), 1)
    } else if delta < 3600 {
        gettext("{}m ago").replacen("{}", &(delta / 60).to_string(), 1)
    } else if delta < 86_400 {
        gettext("{}h ago").replacen("{}", &(delta / 3600).to_string(), 1)
    } else {
        gettext("{}d ago").replacen("{}", &(delta / 86_400).to_string(), 1)
    };

    gettext("Last played: {}").replacen("{}", &ago, 1)
}

#[cfg(test)]
mod tests {
    use super::{duration_brief, playtime};

    #[test]
    fn durations_use_the_largest_units_that_fit() {
        assert_eq!(duration_brief(42), "42s");
        assert_eq!(duration_brief(125), "2m 5s");
        assert_eq!(duration_brief(3 * 3600 + 7 * 60 + 9), "3h 7m");
    }

    #[test]
    fn playtime_drops_to_seconds_only_under_a_minute() {
        assert_eq!(playtime(59), "Playtime: 59s");
        assert_eq!(playtime(61), "Playtime: 1m");
        assert_eq!(playtime(3660), "Playtime: 1h 1m");
    }
}
