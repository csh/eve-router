//! The routing overlay: what it shows, when it shows and where it sits. The small window that
//! shows the next manual hop is in the app crate. The code that reads the title of the foreground
//! window is there too.

use crate::config::{Pos, RoutingOverlayConfig};
use crate::esi::active::{ActiveRoute, Hop};
use crate::labels::LinkView;
use crate::universe::Clade;

/// The title of the window of a logged-in client. The name of the character follows. A client that
/// still loads has the title "EVE", and the launcher has "EVE Launcher". Neither one matches.
const CLIENT_TITLE_PREFIX: &str = "EVE - ";

/// The character name in the title of a client window. A title that is not the title of a
/// logged-in client gives `None`.
pub fn parse_client_title(title: &str) -> Option<&str> {
    let name = title.strip_prefix(CLIENT_TITLE_PREFIX)?;
    (!name.trim().is_empty()).then_some(name)
}

/// What the pilot does at the hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Warp to the signature. The letters are those of the signature in the system that the pilot leaves.
    Signature(String),
    /// No source knows the signature. The pilot looks in the bookmarks of the game.
    CheckBookmarks,
    /// Activate a filament. The clade is `None` for a route file of an older version.
    Filament(Option<Clade>),
}

/// The next manual hop of a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    /// The system that the hop leaves.
    pub from: u32,
    /// The system that the hop enters.
    pub to: u32,
    pub action: Action,
}

/// The next wormhole or filament of the route, but only while the pilot is in the system that the
/// hop leaves (`here`). The overlay helps with a departure and with nothing else. With no known
/// location, or in any other system, the result is `None`. A jump bridge has no text, because the
/// autopilot or the pilot takes it with no help.
pub fn content(route: &ActiveRoute, here: Option<u32>) -> Option<Content> {
    let step = route.next_manual_hop()?;
    let from = route.steps[step - 1].system;
    if here != Some(from) {
        return None;
    }
    let hop = &route.steps[step];
    let action = match hop.hop {
        Hop::Wormhole => match hop.sig.as_deref().filter(|s| !s.is_empty()) {
            Some(letters) => Action::Signature(letters.to_owned()),
            None => Action::CheckBookmarks,
        },
        Hop::Filament => Action::Filament(match hop.link {
            Some(LinkView::Filament { clade, .. }) => Some(clade),
            _ => None,
        }),
        _ => return None,
    };
    Some(Content { from, to: hop.system, action })
}

impl Content {
    /// The two lines of the overlay: the text, and a muted line under it. `name` gives the name of a system.
    pub fn lines(&self, name: impl Fn(u32) -> String) -> (String, String) {
        let hop = format!("{} to {}", name(self.from), name(self.to));
        match &self.action {
            Action::Signature(letters) => (format!("Signature: {letters}"), hop),
            Action::CheckBookmarks => ("Signature: Unknown.".into(), "Check bookmarks.".into()),
            Action::Filament(Some(clade)) => (format!("Activate {} filament", clade.label()), hop),
            Action::Filament(None) => ("Activate a filament".into(), hop),
        }
    }
}

/// What to show for the title of the foreground window. The result is `Some` only when the title
/// is the title of a logged-in client, the character of that client is the character of the route,
/// and the pilot is in the system that the next manual hop leaves (`here`).
pub fn content_for_title(title: &str, route: Option<&ActiveRoute>, here: Option<u32>) -> Option<Content> {
    let route = route?;
    (parse_client_title(title)? == route.character_name).then(|| content(route, here)).flatten()
}

/// The place of the overlay for a character that has none of its own, with no global place.
pub const DEFAULT_POS: Pos = Pos { x: 40, y: 40 };

/// Where the overlay sits for a character: the own position of the character, else the global
/// position. `None` means that the user set neither, so the app uses its built-in place.
pub fn resolve_position(config: &RoutingOverlayConfig, character: u64) -> Option<Pos> {
    config.characters.get(&character).copied().or(config.position)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::esi::active::Step;

    /// A route over the systems 1 to 5. `hops` gives the hop that enters a system, and `sig` the
    /// signature of the wormhole that enters it.
    fn route(hops: &[(u32, Hop, Option<&str>)]) -> ActiveRoute {
        let steps: Vec<Step> = (1..=5u32)
            .map(|system| match hops.iter().find(|(s, ..)| *s == system) {
                Some(&(_, hop, sig)) => Step { system, hop, link: None, sig: sig.map(str::to_owned) },
                None if system == 1 => Step { system, hop: Hop::Start, link: None, sig: None },
                None => Step { system, hop: Hop::Gate, link: None, sig: None },
            })
            .collect();
        ActiveRoute::from_steps(steps, vec![0, 4], 1, 7, "smrkn")
    }

    #[test]
    fn a_logged_in_client_gives_the_character() {
        assert_eq!(parse_client_title("EVE - smrkn"), Some("smrkn"));
        assert_eq!(parse_client_title("EVE - Mary-Jane O'Neil"), Some("Mary-Jane O'Neil"));
        assert_eq!(parse_client_title("EVE - A - B"), Some("A - B"));
    }

    #[test]
    fn the_launcher_the_loading_client_and_other_titles_give_nothing() {
        for title in ["EVE", "EVE Launcher", "EVE - ", "EVE -  ", "", "eve - smrkn", "Chrome - EVE - smrkn", "EVE-smrkn"] {
            assert_eq!(parse_client_title(title), None, "{title:?}");
        }
    }

    /// The names that the label tests use.
    fn name(id: u32) -> String {
        format!("S{id}")
    }

    /// The text of the overlay for a route, with the pilot in system `here`.
    fn lines(r: &ActiveRoute, here: u32) -> Option<(String, String)> {
        content(r, Some(here)).map(|c| c.lines(name))
    }

    #[test]
    fn a_known_signature_shows_in_the_departure_system() {
        let r = route(&[(4, Hop::Wormhole, Some("ABC"))]);
        assert_eq!(content(&r, Some(3)), Some(Content { from: 3, to: 4, action: Action::Signature("ABC".into()) }));
        assert_eq!(lines(&r, 3), Some(("Signature: ABC".into(), "S3 to S4".into())));
    }

    #[test]
    fn nothing_shows_outside_the_departure_system() {
        let r = route(&[(4, Hop::Wormhole, Some("ABC"))]);
        for here in [1, 2, 4, 5, 99] {
            assert_eq!(content(&r, Some(here)), None, "system {here}");
        }
        assert_eq!(content(&r, None), None, "location not known");
    }

    #[test]
    fn an_unknown_signature_says_to_check_the_bookmarks() {
        let r = route(&[(4, Hop::Wormhole, None)]);
        assert_eq!(content(&r, Some(3)), Some(Content { from: 3, to: 4, action: Action::CheckBookmarks }));
        assert_eq!(lines(&r, 3), Some(("Signature: Unknown.".into(), "Check bookmarks.".into())));
        assert_eq!(content(&r, Some(2)), None);
    }

    #[test]
    fn an_empty_signature_counts_as_unknown() {
        let r = route(&[(4, Hop::Wormhole, Some(""))]);
        assert_eq!(content(&r, Some(3)).map(|c| c.action), Some(Action::CheckBookmarks));
    }

    #[test]
    fn the_pilot_who_took_the_wormhole_sees_nothing() {
        let mut r = route(&[(4, Hop::Wormhole, Some("ABC"))]);
        r.progress = 3;
        assert_eq!(content(&r, Some(3)), None);
        assert_eq!(content(&r, Some(4)), None);
    }

    #[test]
    fn a_route_with_gates_only_shows_nothing() {
        let r = route(&[]);
        for here in 1..=5 {
            assert_eq!(content(&r, Some(here)), None);
        }
    }

    #[test]
    fn a_jump_bridge_shows_nothing() {
        let r = route(&[(3, Hop::Bridge, None)]);
        for here in 1..=5 {
            assert_eq!(content(&r, Some(here)), None);
        }
    }

    #[test]
    fn a_jump_bridge_does_not_hide_the_wormhole_behind_it() {
        let r = route(&[(2, Hop::Bridge, None), (5, Hop::Wormhole, Some("XYZ"))]);
        assert_eq!(content(&r, Some(4)), Some(Content { from: 4, to: 5, action: Action::Signature("XYZ".into()) }));
        assert_eq!(content(&r, Some(1)), None);
    }

    /// A route with a filament that enters system 3.
    fn filament_route(clade: Option<Clade>) -> ActiveRoute {
        let mut r = route(&[(3, Hop::Filament, None)]);
        r.steps[2].link = clade.map(|clade| LinkView::Filament { clade, expected_jumps: Some(4.0) });
        r
    }

    #[test]
    fn a_filament_shows_in_the_departure_system() {
        let r = filament_route(Some(Clade::Perun));
        assert_eq!(content(&r, Some(1)), None);
        assert_eq!(content(&r, Some(2)), Some(Content { from: 2, to: 3, action: Action::Filament(Some(Clade::Perun)) }));
    }

    #[test]
    fn a_filament_names_its_clade() {
        let r = filament_route(Some(Clade::Svarog));
        assert_eq!(lines(&r, 2), Some(("Activate Krai Svarog filament".into(), "S2 to S3".into())));
    }

    #[test]
    fn a_filament_from_an_old_route_file_has_no_clade() {
        let r = filament_route(None);
        assert_eq!(lines(&r, 2), Some(("Activate a filament".into(), "S2 to S3".into())));
    }

    #[test]
    fn a_filament_ahead_hides_the_wormhole_behind_it() {
        let r = route(&[(3, Hop::Filament, None), (5, Hop::Wormhole, Some("XYZ"))]);
        assert_eq!(content(&r, Some(4)), None);
    }

    #[test]
    fn the_next_wormhole_wins_over_a_later_one() {
        let mut r = route(&[(3, Hop::Wormhole, Some("AAA")), (5, Hop::Wormhole, Some("BBB"))]);
        assert_eq!(content(&r, Some(2)).map(|c| c.action), Some(Action::Signature("AAA".into())));
        assert_eq!(content(&r, Some(4)), None);
        r.progress = 2;
        assert_eq!(content(&r, Some(4)), Some(Content { from: 4, to: 5, action: Action::Signature("BBB".into()) }));
    }

    #[test]
    fn only_the_client_of_the_route_character_shows_the_overlay() {
        let r = route(&[(4, Hop::Wormhole, Some("ABC"))]);
        assert!(content_for_title("EVE - smrkn", Some(&r), Some(3)).is_some());
        assert_eq!(content_for_title("EVE - smrkn", Some(&r), Some(2)), None);
        assert_eq!(content_for_title("EVE - Alice", Some(&r), Some(3)), None);
        assert_eq!(content_for_title("EVE", Some(&r), Some(3)), None);
        assert_eq!(content_for_title("EVE Launcher", Some(&r), Some(3)), None);
        assert_eq!(content_for_title("EVE - smrkn", None, Some(3)), None);
    }

    #[test]
    fn the_position_of_a_character_wins_over_the_global_position() {
        let mut config = RoutingOverlayConfig::default();
        assert_eq!(resolve_position(&config, 7), None);
        config.position = Some(Pos { x: 10, y: 20 });
        assert_eq!(resolve_position(&config, 7), Some(Pos { x: 10, y: 20 }));
        config.characters.insert(7, Pos { x: -5, y: 8 });
        assert_eq!(resolve_position(&config, 7), Some(Pos { x: -5, y: 8 }));
        assert_eq!(resolve_position(&config, 8), Some(Pos { x: 10, y: 20 }));
    }
}
