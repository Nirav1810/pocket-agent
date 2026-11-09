# Privacy

Pocket Agent has no servers, accounts, analytics, telemetry or crash reporting.
The project never receives any of your data.

## What leaves your phone, and where it goes

Only while a command is running:

| Data | Sent to | Why |
|---|---|---|
| The text of your spoken command (after on-device or system speech recognition) | Your relay → `api.anthropic.com`, or directly to `api.anthropic.com` | So Claude can decide what to do |
| A pruned text description of the current screen (labels, button text; password fields redacted) during screen tasks | Same | So Claude can operate the app |
| Your preferred apps and language, and preferences you choose to save | Same (in the prompt); optionally your own relay's `/v1/prefs` | Personalisation |

Nothing else, to anywhere. No screenshots or images are sent.

## Speech

Speech recognition uses Android's `SpeechRecognizer`, preferring on-device
recognition. Depending on your phone, the system recognizer may itself use a
network service run by your phone's vendor; that is outside Pocket Agent's
control and governed by your system settings. Text-to-speech uses the system
engine.

## What your relay stores (in your Cloudflare D1 database)

- Paired devices: id, name, SHA-256 hash of the token, created/last-seen times.
- Usage: per day, device and model: token counts, estimated cost, request count.
- Preferences blob (if the app syncs it).

It never stores or logs the content of requests or responses.

## What Anthropic receives

The requests above, under your API key and your agreement with Anthropic. See
Anthropic's privacy policy and your Console data-retention settings.

## On the phone

Settings and preferences in app-private storage; the API key or device token
encrypted with an Android Keystore key. Task history is kept in memory only and
discarded when a task ends. `allowBackup` is off, so nothing goes to cloud
backups.
