//! Spoken messages the agent generates itself (errors, confirmations).
//!
//! Most spoken text is written by Claude in the user's language (the `say`
//! field of each tool). These templates cover what Rust must say on its own:
//! errors when Claude is unreachable, and confirmation prompts. English is the
//! fallback for languages without a translation. Contributors: add a language
//! by adding a `match` arm in [`table`]; keep messages short (they are spoken).
//!
//! Placeholders: `{target}`, `{app}`, `{contact}`, `{text}`, `{label}`.

/// Message keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    /// No credentials.
    NotConfigured,
    /// No network.
    Offline,
    /// Request timed out.
    Timeout,
    /// 429/529 after retries.
    Busy,
    /// 5xx after retries.
    ServerError,
    /// Relay rejected this device.
    DeviceUnauthorized,
    /// Direct-mode key rejected.
    KeyRejectedDirect,
    /// Relay's key rejected by Anthropic.
    KeyRejectedRelay,
    /// Relay spending cap.
    RelayCap,
    /// Relay model allowlist.
    ModelNotAllowed,
    /// Local daily spending limit.
    DailyLimit,
    /// Local monthly spending limit.
    MonthlyLimit,
    /// Claude declined.
    Refused,
    /// Unexpected failure.
    Generic,
    /// User said no.
    Cancelled,
    /// User pressed STOP.
    Stopped,
    /// Confirm a call.
    ConfirmCall,
    /// Confirm a message.
    ConfirmMessage,
    /// Confirm a sensitive tap.
    ConfirmTap,
    /// Confirm a tap in a payment app.
    ConfirmPayment,
    /// Confirm a security-settings change.
    ConfirmSecurity,
    /// Confirm (generic).
    ConfirmGeneric,
    /// App is blocklisted.
    Blocked,
    /// Intent could not be launched.
    ActionFailed,
    /// Screen control not available yet.
    ScreenUnsupported,
    /// Message drafted, user must press send.
    MessageDrafted,
    /// Generic success.
    Done,
}

/// Primary language subtag of a BCP-47 tag (`hi-IN` → `hi`).
pub fn lang(locale: &str) -> String {
    locale
        .split(['-', '_'])
        .next()
        .unwrap_or("en")
        .to_ascii_lowercase()
}

/// Localised message with placeholders filled.
pub fn t(locale: &str, msg: Msg, args: &[(&str, &str)]) -> String {
    let template = table(&lang(locale), msg).unwrap_or_else(|| english(msg));
    let mut out = template.to_owned();
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

fn table(lang: &str, msg: Msg) -> Option<&'static str> {
    match lang {
        "en" => Some(english(msg)),
        "hi" => hindi(msg),
        "es" => spanish(msg),
        _ => None,
    }
}

fn english(msg: Msg) -> &'static str {
    use Msg::*;
    match msg {
        NotConfigured => "I'm not set up yet. Open Pocket Agent to connect your Claude account.",
        Offline => "I can't reach the internet right now.",
        Timeout => "That took too long. Please try again.",
        Busy => "Claude is busy right now. Please try again in a moment.",
        ServerError => "Something went wrong on the server. Please try again.",
        DeviceUnauthorized => {
            "This phone is no longer paired with your relay. Please pair it again."
        }
        KeyRejectedDirect => "Your API key was rejected. Please check it in settings.",
        KeyRejectedRelay => "Your relay's API key was rejected. Update it on the relay.",
        RelayCap => "Your relay's spending limit has been reached.",
        ModelNotAllowed => "Your relay doesn't allow the selected model.",
        DailyLimit => "You've reached today's spending limit. You can raise it in settings.",
        MonthlyLimit => "You've reached this month's spending limit. You can raise it in settings.",
        Refused => "Sorry, I can't help with that.",
        Generic => "Sorry, something went wrong.",
        Cancelled => "Okay, cancelled.",
        Stopped => "Stopped.",
        ConfirmCall => "Call {target}?",
        ConfirmMessage => "Send to {contact} on {app}: \"{text}\"?",
        ConfirmTap => "Tap \"{label}\"?",
        ConfirmPayment => "This is a payment app. Tap \"{label}\"?",
        ConfirmSecurity => "This changes a security setting. Continue with \"{label}\"?",
        ConfirmGeneric => "Go ahead?",
        Blocked => "I'm not allowed to use {app}. You can change this in settings.",
        ActionFailed => "I couldn't do that on this phone.",
        ScreenUnsupported => "I can't operate the screen yet.",
        MessageDrafted => "Your message is ready. Tap send to deliver it.",
        Done => "Done.",
    }
}

// Translations below need native-speaker review (see CONTRIBUTING.md).

fn hindi(msg: Msg) -> Option<&'static str> {
    use Msg::*;
    Some(match msg {
        NotConfigured => "मैं अभी सेट अप नहीं हूँ। अपना Claude खाता जोड़ने के लिए Pocket Agent खोलें।",
        Offline => "मैं अभी इंटरनेट से नहीं जुड़ पा रहा हूँ।",
        Timeout => "बहुत समय लग गया। कृपया फिर से कोशिश करें।",
        Busy => "Claude अभी व्यस्त है। थोड़ी देर बाद कोशिश करें।",
        ServerError => "सर्वर पर कुछ गड़बड़ हुई। कृपया फिर से कोशिश करें।",
        DeviceUnauthorized => "यह फ़ोन अब आपके रिले से जुड़ा नहीं है। कृपया फिर से पेयर करें।",
        KeyRejectedDirect => "आपकी API कुंजी अस्वीकार हो गई। सेटिंग्स में जाँचें।",
        KeyRejectedRelay => "आपके रिले की API कुंजी अस्वीकार हो गई। रिले पर इसे अपडेट करें।",
        RelayCap => "आपके रिले की खर्च सीमा पूरी हो गई है।",
        ModelNotAllowed => "आपका रिले चुने गए मॉडल की अनुमति नहीं देता।",
        DailyLimit => "आज की खर्च सीमा पूरी हो गई है। आप इसे सेटिंग्स में बढ़ा सकते हैं।",
        MonthlyLimit => "इस महीने की खर्च सीमा पूरी हो गई है। आप इसे सेटिंग्स में बढ़ा सकते हैं।",
        Refused => "माफ़ कीजिए, मैं इसमें मदद नहीं कर सकता।",
        Generic => "माफ़ कीजिए, कुछ गड़बड़ हो गई।",
        Cancelled => "ठीक है, रद्द कर दिया।",
        Stopped => "रोक दिया।",
        ConfirmCall => "{target} को कॉल करें?",
        ConfirmMessage => "{app} पर {contact} को भेजें: \"{text}\"?",
        ConfirmTap => "\"{label}\" दबाएँ?",
        ConfirmPayment => "यह भुगतान ऐप है। \"{label}\" दबाएँ?",
        ConfirmSecurity => "इससे सुरक्षा सेटिंग बदलेगी। \"{label}\" जारी रखें?",
        ConfirmGeneric => "आगे बढ़ें?",
        Blocked => "मुझे {app} इस्तेमाल करने की अनुमति नहीं है। आप इसे सेटिंग्स में बदल सकते हैं।",
        ActionFailed => "मैं इस फ़ोन पर यह नहीं कर सका।",
        ScreenUnsupported => "मैं अभी स्क्रीन नहीं चला सकता।",
        MessageDrafted => "आपका संदेश तैयार है। भेजने के लिए सेंड दबाएँ।",
        Done => "हो गया।",
    })
}

fn spanish(msg: Msg) -> Option<&'static str> {
    use Msg::*;
    Some(match msg {
        NotConfigured => {
            "Todavía no estoy configurado. Abre Pocket Agent para conectar tu cuenta de Claude."
        }
        Offline => "No puedo conectarme a internet ahora mismo.",
        Timeout => "Tardó demasiado. Inténtalo de nuevo.",
        Busy => "Claude está ocupado. Inténtalo en un momento.",
        ServerError => "Algo falló en el servidor. Inténtalo de nuevo.",
        DeviceUnauthorized => "Este teléfono ya no está vinculado a tu relay. Vuelve a vincularlo.",
        KeyRejectedDirect => "Tu clave de API fue rechazada. Revísala en ajustes.",
        KeyRejectedRelay => "La clave de API de tu relay fue rechazada. Actualízala en el relay.",
        RelayCap => "Se alcanzó el límite de gasto de tu relay.",
        ModelNotAllowed => "Tu relay no permite el modelo seleccionado.",
        DailyLimit => "Llegaste al límite de gasto de hoy. Puedes subirlo en ajustes.",
        MonthlyLimit => "Llegaste al límite de gasto de este mes. Puedes subirlo en ajustes.",
        Refused => "Lo siento, no puedo ayudar con eso.",
        Generic => "Lo siento, algo salió mal.",
        Cancelled => "Vale, cancelado.",
        Stopped => "Detenido.",
        ConfirmCall => "¿Llamar a {target}?",
        ConfirmMessage => "¿Enviar a {contact} por {app}: \"{text}\"?",
        ConfirmTap => "¿Pulsar \"{label}\"?",
        ConfirmPayment => "Esta es una app de pagos. ¿Pulsar \"{label}\"?",
        ConfirmSecurity => "Esto cambia un ajuste de seguridad. ¿Continuar con \"{label}\"?",
        ConfirmGeneric => "¿Continuar?",
        Blocked => "No tengo permitido usar {app}. Puedes cambiarlo en ajustes.",
        ActionFailed => "No pude hacerlo en este teléfono.",
        ScreenUnsupported => "Todavía no puedo manejar la pantalla.",
        MessageDrafted => "Tu mensaje está listo. Pulsa enviar para mandarlo.",
        Done => "Hecho.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_english() {
        assert_eq!(t("ta-IN", Msg::Cancelled, &[]), "Okay, cancelled.");
        assert_eq!(t("hi-IN", Msg::Cancelled, &[]), "ठीक है, रद्द कर दिया।");
        assert_eq!(t("es_MX", Msg::Done, &[]), "Hecho.");
    }

    #[test]
    fn placeholders() {
        assert_eq!(t("en", Msg::ConfirmCall, &[("target", "Mom")]), "Call Mom?");
        assert_eq!(
            t(
                "en",
                Msg::ConfirmMessage,
                &[("contact", "Rahul"), ("app", "WhatsApp"), ("text", "late")]
            ),
            "Send to Rahul on WhatsApp: \"late\"?"
        );
    }

    #[test]
    fn every_translation_keeps_placeholders() {
        let all = [
            Msg::ConfirmCall,
            Msg::ConfirmMessage,
            Msg::ConfirmTap,
            Msg::ConfirmPayment,
            Msg::ConfirmSecurity,
            Msg::Blocked,
        ];
        for l in ["hi", "es"] {
            for m in all {
                let en = english(m);
                let tr = table(l, m).unwrap();
                for ph in ["{target}", "{app}", "{contact}", "{text}", "{label}"] {
                    assert_eq!(en.contains(ph), tr.contains(ph), "{l} {m:?} {ph}");
                }
            }
        }
    }
}
