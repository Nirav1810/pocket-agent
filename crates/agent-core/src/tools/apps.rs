//! Resolving spoken app names to installed packages.
//!
//! The model never supplies package names; it says "Spotify" and we match it
//! against `installed_apps()` here. Nothing is guessed: no match → error.

use crate::types::AppInfo;

/// Spoken names that differ from launcher labels. Keys and values are
/// normalised (lowercase, alphanumerics only).
const ALIASES: &[(&str, &str)] = &[
    ("youtubemusic", "ytmusic"),
    ("googlemaps", "maps"),
    ("gmaps", "maps"),
    ("googlechrome", "chrome"),
    ("playstore", "googleplaystore"),
    ("sms", "messages"),
    ("textmessage", "messages"),
    ("insta", "instagram"),
    ("fb", "facebook"),
    ("whatsappbusiness", "wabusiness"),
];

/// Lowercase and keep only letters/digits (any script).
pub fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Best installed app for a spoken name, or `None`.
///
/// Order: exact label → alias → label prefix/word → substring (≥ 3 chars) →
/// package segment. Ties prefer the shortest label ("Maps" over "Maps Go").
pub fn find<'a>(apps: &'a [AppInfo], spoken: &str) -> Option<&'a AppInfo> {
    let want = normalize(spoken.trim_end_matches(" app").trim());
    if want.is_empty() {
        return None;
    }
    let alias = ALIASES.iter().find(|(k, _)| *k == want).map(|(_, v)| *v);
    let shortest = |it: Vec<&'a AppInfo>| it.into_iter().min_by_key(|a| a.label.chars().count());
    let label = |a: &AppInfo| normalize(&a.label);

    let exact: Vec<_> = apps
        .iter()
        .filter(|a| label(a) == want || alias.is_some_and(|al| label(a) == al))
        .collect();
    if let Some(a) = shortest(exact) {
        return Some(a);
    }
    let words: Vec<_> = apps
        .iter()
        .filter(|a| {
            a.label.split_whitespace().any(|w| normalize(w) == want) || label(a).starts_with(&want)
        })
        .collect();
    if let Some(a) = shortest(words) {
        return Some(a);
    }
    if want.chars().count() >= 3 {
        let contains: Vec<_> = apps.iter().filter(|a| label(a).contains(&want)).collect();
        if let Some(a) = shortest(contains) {
            return Some(a);
        }
    }
    let by_pkg: Vec<_> = apps
        .iter()
        .filter(|a| a.package.split('.').any(|seg| normalize(seg) == want))
        .collect();
    shortest(by_pkg)
}

/// Comma-separated closest labels, for error messages to the model.
pub fn suggestions(apps: &[AppInfo], spoken: &str, max: usize) -> String {
    let want = normalize(spoken);
    let first: String = want.chars().take(2).collect();
    let mut s: Vec<&str> = apps
        .iter()
        .filter(|a| !first.is_empty() && normalize(&a.label).starts_with(&first))
        .map(|a| a.label.as_str())
        .take(max)
        .collect();
    s.sort_unstable();
    s.join(", ")
}

#[cfg(test)]
pub(crate) fn sample_apps() -> Vec<AppInfo> {
    [
        ("com.google.android.apps.youtube.music", "YT Music"),
        ("com.spotify.music", "Spotify"),
        ("com.google.android.apps.maps", "Maps"),
        ("com.google.android.apps.mapslite", "Maps Go"),
        ("net.osmand", "OsmAnd"),
        ("com.whatsapp", "WhatsApp"),
        ("com.whatsapp.w4b", "WA Business"),
        ("in.swiggy.android", "Swiggy"),
        ("com.android.chrome", "Chrome"),
        ("com.google.android.apps.messaging", "Messages"),
        ("com.google.android.deskclock", "Clock"),
        ("org.telegram.messenger", "Telegram"),
        ("com.instagram.android", "Instagram"),
    ]
    .iter()
    .map(|(p, l)| AppInfo {
        package: (*p).into(),
        label: (*l).into(),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str) -> Option<String> {
        find(&sample_apps(), name).map(|a| a.package.clone())
    }

    #[test]
    fn matching() {
        assert_eq!(pkg("Spotify").as_deref(), Some("com.spotify.music"));
        assert_eq!(pkg("spotify app").as_deref(), Some("com.spotify.music"));
        assert_eq!(
            pkg("YouTube Music").as_deref(),
            Some("com.google.android.apps.youtube.music")
        );
        assert_eq!(
            pkg("google maps").as_deref(),
            Some("com.google.android.apps.maps")
        );
        assert_eq!(pkg("maps").as_deref(), Some("com.google.android.apps.maps"));
        assert_eq!(pkg("whats app").as_deref(), Some("com.whatsapp"));
        assert_eq!(pkg("insta").as_deref(), Some("com.instagram.android"));
        assert_eq!(pkg("osmand").as_deref(), Some("net.osmand"));
        assert_eq!(pkg("swig").as_deref(), Some("in.swiggy.android"));
        assert_eq!(pkg("telegram").as_deref(), Some("org.telegram.messenger"));
    }

    #[test]
    fn never_guesses() {
        assert_eq!(pkg("Zomato"), None);
        assert_eq!(pkg(""), None);
        assert_eq!(pkg("ab"), None);
    }

    #[test]
    fn non_latin_normalisation() {
        // Both spellings normalise identically and stay non-empty.
        assert_eq!(normalize("व्हाट्सएप "), normalize("व्हाट्सएप"));
        assert!(!normalize("व्हाट्सएप").is_empty());
        assert_eq!(normalize("Ä-B c"), "äbc");
    }

    #[test]
    fn suggestion_list() {
        assert_eq!(suggestions(&sample_apps(), "Mapz", 5), "Maps, Maps Go");
    }
}
