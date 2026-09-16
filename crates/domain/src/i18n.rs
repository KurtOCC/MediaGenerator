//! Central catalogue of all user-facing text.
//!
//! Every string the end user can see lives here, in Norwegian Bokmål (`nb-NO`),
//! so that the application can be translated later by adding a sibling module
//! and selecting it at runtime. No other module in the workspace should contain
//! user-facing prose — neither in Rust code nor hard-coded in templates.

/// Norwegian Bokmål strings. This is the only locale shipped today.
pub mod nb {
    // ----- Shell / chrome -------------------------------------------------
    /// BCP-47 language tag emitted in `<html lang="…">`.
    pub const LANG: &str = "nb";
    /// Organisation name shown in the top bar.
    pub const BRAND: &str = "Oslofjord IT";
    /// Browser tab title.
    pub const APP_TITLE: &str = "Mediagenerator";
    /// Meta description for the application shell.
    pub const APP_DESCRIPTION: &str = "Lag bilder, lyd og video med AI.";

    /// User menu: link to the signed-in user's own history.
    pub const NAV_MY_HISTORY: &str = "Min historikk";
    /// User menu: sign out.
    pub const NAV_LOGOUT: &str = "Logg ut";
    /// Accessible label for the user menu trigger.
    pub const NAV_USER_MENU_LABEL: &str = "Åpne brukermeny";

    // ----- Hero -----------------------------------------------------------
    /// Small uppercase eyebrow above the headline.
    pub const HERO_EYEBROW: &str = "Generer med AI";
    /// Headline, first half (rendered in plain white).
    pub const HERO_TITLE_LEAD: &str = "Media";
    /// Headline, second half (rendered with the purple→blue gradient).
    pub const HERO_TITLE_ACCENT: &str = "generator";
    /// Ingress, first line.
    pub const HERO_SUBTITLE_1: &str = "Lag bilder, lyd og video med AI.";
    /// Ingress, second line.
    pub const HERO_SUBTITLE_2: &str = "Beskriv hva du ønsker og velg medietype.";

    // ----- Generator card -------------------------------------------------
    /// Placeholder text in the prompt textarea.
    pub const PROMPT_PLACEHOLDER: &str = "Beskriv hva du ønsker å lage…";
    /// Accessible label for the prompt textarea.
    pub const PROMPT_LABEL: &str = "Beskrivelse av det du vil generere";
    /// Label for the media type radio group.
    pub const MEDIA_TYPE_LEGEND: &str = "Velg medietype";
    /// Media type: image.
    pub const MEDIA_IMAGE: &str = "Bilde";
    /// Media type: image, subtitle.
    pub const MEDIA_IMAGE_SUB: &str = "Bilder og illustrasjoner";
    /// Media type: audio.
    pub const MEDIA_AUDIO: &str = "Lyd";
    /// Media type: audio, subtitle.
    pub const MEDIA_AUDIO_SUB: &str = "Tale, musikk og lydeffekter";
    /// Media type: video.
    pub const MEDIA_VIDEO: &str = "Video";
    /// Media type: video, subtitle.
    pub const MEDIA_VIDEO_SUB: &str = "Korte videoklipp";
    /// Primary submit button.
    pub const GENERATE: &str = "Generer";
    /// Primary submit button while a job is running.
    pub const GENERATING: &str = "Genererer…";

    /// Leading label of the suggestion chip row.
    pub const SUGGESTIONS_LABEL: &str = "Forslag:";

    // ----- Result / gallery ----------------------------------------------
    /// Heading of the generated-content section.
    pub const GENERATED_HEADING: &str = "Generert";
    /// Link from the generated section to the archive.
    pub const GENERATED_LINK: &str = "Se ferdig generert innhold";
    /// Download the produced asset.
    pub const DOWNLOAD: &str = "Last ned";
    /// Copy a time-limited link to the produced asset.
    pub const COPY_LINK: &str = "Kopier lenke";
    /// Re-run the same prompt.
    pub const REGENERATE: &str = "Generer på nytt";
    /// Toast shown when a background job finishes.
    pub const TOAST_DONE: &str = "Genereringen er ferdig";

    // ----- Footer ---------------------------------------------------------
    /// Muted footer line.
    pub const FOOTER: &str = "Vibecoding for the win";

    // ----- Errors ---------------------------------------------------------
    /// Generic fallback for unexpected server errors.
    pub const ERR_INTERNAL: &str = "Noe gikk galt hos oss. Prøv igjen om litt – feilen er logget.";
    /// Shown when the session is missing or expired.
    pub const ERR_UNAUTHENTICATED: &str = "Du må logge inn for å bruke Mediagenerator.";
    /// Shown when the user lacks the required app role.
    pub const ERR_FORBIDDEN: &str =
        "Du har ikke tilgang til Mediagenerator. Kontakt Oslofjord IT for å få tilgang.";
    /// Shown when an entity does not exist or belongs to someone else.
    pub const ERR_NOT_FOUND: &str = "Fant ikke det du lette etter.";
    /// Shown when the per-user rate limit is exceeded.
    pub const ERR_RATE_LIMITED: &str =
        "Du har generert mye på kort tid. Vent litt før du prøver igjen.";
    /// Shown when the provider's content filter rejected the prompt.
    pub const ERR_CONTENT_FILTER: &str =
        "Beskrivelsen ble avvist av innholdsfilteret. Prøv å formulere den annerledes.";
    /// Shown when an upstream Azure service failed or timed out.
    pub const ERR_UPSTREAM: &str =
        "AI-tjenesten svarer ikke akkurat nå. Prøv igjen om noen minutter.";
    /// Shown when the CSRF token is missing or does not match.
    pub const ERR_CSRF: &str = "Sikkerhetssjekken feilet. Last siden på nytt og prøv igjen.";
    /// Shown when the prompt is empty.
    pub const ERR_PROMPT_EMPTY: &str = "Du må beskrive hva du vil lage.";
    /// Shown when the prompt contains disallowed control characters.
    pub const ERR_PROMPT_CONTROL_CHARS: &str = "Beskrivelsen inneholder ugyldige tegn.";

    /// Returns the message shown when the prompt exceeds `max` characters.
    pub fn err_prompt_too_long(max: usize) -> String {
        format!("Beskrivelsen kan være maks {max} tegn.")
    }
}
