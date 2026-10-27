// Copyright (C) 2026 AnalyseDeCircuit
// SPDX-License-Identifier: GPL-3.0-only

use oxideterm_settings::UpdateChannel;

pub const STABLE_UPDATE_ENDPOINT: &str =
    "https://github.com/NevermoreDDD/oxideterm/releases/latest/download/latest.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpdateEndpoint {
    pub channel: UpdateChannel,
    pub url: &'static str,
}

pub fn endpoint_for_channel(_channel: UpdateChannel) -> UpdateEndpoint {
    // The fork publishes only a stable feed; existing beta settings must not
    // send update checks to a missing feed.
    UpdateEndpoint {
        channel: UpdateChannel::Stable,
        url: STABLE_UPDATE_ENDPOINT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_beta_settings_check_the_fork_stable_feed() {
        for channel in [UpdateChannel::Stable, UpdateChannel::Beta] {
            assert_eq!(
                endpoint_for_channel(channel),
                UpdateEndpoint {
                    channel: UpdateChannel::Stable,
                    url: "https://github.com/NevermoreDDD/oxideterm/releases/latest/download/latest.json",
                }
            );
        }
    }
}
