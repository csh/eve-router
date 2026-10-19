//! The routing overlay: what it shows, when it shows and where it sits. The small window that
//! shows the signature of the next wormhole is in the app crate. The code that reads the title of
//! the foreground window is there too.

use crate::config::{Pos, RoutingOverlayConfig};
use crate::esi::active::{ActiveRoute, Hop};

/// The title of the window of a logged-in client. The name of the character follows. A client that
/// still loads has the title "EVE", and the launcher has "EVE Launcher". Neither one matches.
const CLIENT_TITLE_PREFIX: &str = "EVE - ";

/// The character name in the title of a client window. A title that is not the title of a
/// logged-in client gives `None`.
pub fn parse_client_title(title: &str) -> Option<&str> {
    let name = title.strip_prefix(CLIENT_TITLE_PREFIX)?;
    (!name.trim().is_empty()).then_some(name)
}

/// What the overlay tells the pilot about the signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signature {
    /// The letters of the signature in the system that the pilot leaves.
    Known(String),
    /// No source knows the signature. The pilot looks in the bookmarks of the game.
    CheckBookmarks,
}

/// The next wormhole of a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    /// The system that holds the signature.
    pub from: u32,
    /// The system on the far side of the wormhole.
    pub to: u32,
    pub signature: Signature,
}

/// The next wormhole of the route, when the overlay has something to say about it. A wormhole
/// with no known signature gives a result only when the pilot is in the system that it leaves.
/// A filament, or a route with no wormhole ahead, gives `None`.
pub fn content(route: &ActiveRoute) -> Option<Content> {
    let step = route.next_manual_hop().filter(|&i| route.steps[i].hop == Hop::Wormhole)?;
    let at_departure = step == route.progress + 1;
    let signature = match route.steps[step].sig.as_deref().filter(|s| !s.is_empty()) {
        Some(letters) => Signature::Known(letters.to_owned()),
        None if at_departure => Signature::CheckBookmarks,
        None => return None,
    };
    Some(Content { from: route.steps[step - 1].system, to: route.steps[step].system, signature })
}

/// What to show for the title of the foreground window. The result is `Some` only when the title
/// is the title of a logged-in client, and the character of that client is the character of the route.
pub fn content_for_title(title: &str, route: Option<&ActiveRoute>) -> Option<Content> {
    let route = route?;
    (parse_client_title(title)? == route.character_name).then(|| content(route)).flatten()
}

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

    #[test]
    fn a_known_signature_shows_before_the_pilot_arrives() {
        let r = route(&[(4, Hop::Wormhole, Some("ABC"))]);
        assert_eq!(r.progress, 0);
        assert_eq!(content(&r), Some(Content { from: 3, to: 4, signature: Signature::Known("ABC".into()) }));
    }

    #[test]
    fn an_unknown_signature_shows_only_at_the_departure_system() {
        let mut r = route(&[(4, Hop::Wormhole, None)]);
        assert_eq!(content(&r), None);
        r.progress = 1;
        assert_eq!(content(&r), None);
        r.progress = 2;
        assert_eq!(content(&r), Some(Content { from: 3, to: 4, signature: Signature::CheckBookmarks }));
    }

    #[test]
    fn an_empty_signature_counts_as_unknown() {
        let mut r = route(&[(4, Hop::Wormhole, Some(""))]);
        r.progress = 2;
        assert_eq!(content(&r).map(|c| c.signature), Some(Signature::CheckBookmarks));
    }

    #[test]
    fn the_pilot_who_took_the_wormhole_sees_nothing() {
        let mut r = route(&[(4, Hop::Wormhole, Some("ABC"))]);
        r.progress = 3;
        assert_eq!(content(&r), None);
    }

    #[test]
    fn a_route_with_gates_only_shows_nothing() {
        assert_eq!(content(&route(&[])), None);
    }

    #[test]
    fn a_filament_ahead_shows_nothing_even_with_a_wormhole_behind_it() {
        let r = route(&[(3, Hop::Filament, None), (5, Hop::Wormhole, Some("XYZ"))]);
        assert_eq!(content(&r), None);
    }

    #[test]
    fn the_next_wormhole_wins_over_a_later_one() {
        let mut r = route(&[(3, Hop::Wormhole, Some("AAA")), (5, Hop::Wormhole, Some("BBB"))]);
        assert_eq!(content(&r).map(|c| c.signature), Some(Signature::Known("AAA".into())));
        r.progress = 2;
        assert_eq!(content(&r), Some(Content { from: 4, to: 5, signature: Signature::Known("BBB".into()) }));
    }

    #[test]
    fn only_the_client_of_the_route_character_shows_the_overlay() {
        let r = route(&[(4, Hop::Wormhole, Some("ABC"))]);
        assert!(content_for_title("EVE - smrkn", Some(&r)).is_some());
        assert_eq!(content_for_title("EVE - Alice", Some(&r)), None);
        assert_eq!(content_for_title("EVE", Some(&r)), None);
        assert_eq!(content_for_title("EVE Launcher", Some(&r)), None);
        assert_eq!(content_for_title("EVE - smrkn", None), None);
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
