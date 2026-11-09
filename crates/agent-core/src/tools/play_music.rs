//! `play_music(query, app?)` → `MEDIA_PLAY_FROM_SEARCH`.

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, apps, opt_text, parse, say, schema, text};
use crate::types::IntentSpec;

/// Play a song, artist, album, playlist or podcast.
pub struct PlayMusic;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    query: String,
    app: Option<String>,
    say: String,
}

impl Tool for PlayMusic {
    fn name(&self) -> &'static str {
        "play_music"
    }

    fn description(&self) -> &'static str {
        "Play music or audio: a song, artist, album, genre, playlist or podcast. \
         Set `app` only if the user named a specific app (e.g. \"on Spotify\"); \
         otherwise null and the user's preferred music app is used."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            (
                "query",
                schema::string("What to play, e.g. \"Arijit Singh\" or \"lo-fi beats playlist\"."),
            ),
            (
                "app",
                schema::nullable_string(
                    "App the user explicitly named, e.g. \"YouTube Music\", else null.",
                ),
            ),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        let query = text("query", &i.query, 200)?;
        let package = match opt_text("app", i.app.as_deref(), 60)? {
            Some(name) => Some(
                apps::find(ctx.apps(), &name)
                    .map(|a| a.package.clone())
                    .ok_or_else(|| {
                        ToolError::NotFound(format!("no installed app matches \"{name}\""))
                    })?,
            ),
            None => ctx.settings.music_app.clone(),
        };
        Ok(ToolPlan::Intent {
            spec: IntentSpec::PlayMusic { query, package },
            say: say(&i.say),
            follow_up: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::tools::apps::sample_apps;
    use serde_json::json;

    #[test]
    fn named_app_resolves_to_package() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, sample_apps());
        let p = PlayMusic
            .plan(
                &json!({"query":"Arijit Singh","app":"YouTube Music","say":"Playing."}),
                &ctx,
            )
            .unwrap();
        assert_eq!(
            p,
            ToolPlan::Intent {
                spec: IntentSpec::PlayMusic {
                    query: "Arijit Singh".into(),
                    package: Some("com.google.android.apps.youtube.music".into())
                },
                say: "Playing.".into(),
                follow_up: None
            }
        );
    }

    #[test]
    fn default_app_from_settings() {
        let s = Settings {
            music_app: Some("com.spotify.music".into()),
            ..Default::default()
        };
        let ctx = ToolCtx::with_apps(&s, sample_apps());
        let p = PlayMusic
            .plan(&json!({"query":"jazz","app":null,"say":"ok"}), &ctx)
            .unwrap();
        assert!(
            matches!(p, ToolPlan::Intent { spec: IntentSpec::PlayMusic { package: Some(ref p), .. }, .. } if p == "com.spotify.music")
        );
    }

    #[test]
    fn unknown_app_and_bad_input() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, sample_apps());
        assert!(matches!(
            PlayMusic.plan(&json!({"query":"x","app":"Gaana","say":""}), &ctx),
            Err(ToolError::NotFound(_))
        ));
        assert!(matches!(
            PlayMusic.plan(&json!({"query":" ","app":null,"say":""}), &ctx),
            Err(ToolError::Invalid(_))
        ));
        assert!(matches!(
            PlayMusic.plan(&json!({"query":"x","app":null,"say":"","extra":1}), &ctx),
            Err(ToolError::Invalid(_))
        ));
    }
}
