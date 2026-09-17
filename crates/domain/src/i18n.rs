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
    /// Sign in with the corporate identity provider.
    pub const NAV_LOGIN: &str = "Logg inn med Oslofjord-konto";
    /// Accessible label for the user menu trigger.
    pub const NAV_USER_MENU_LABEL: &str = "Åpne brukermeny";
    /// Fallback display name when the ID token carried no usable name.
    pub const UNKNOWN_USER: &str = "Ukjent bruker";

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

    // ----- Model options ---------------------------------------------------
    /// Heading above the per-media-type options.
    pub const OPTIONS_LABEL: &str = "Valg";

    /// Image: label for the size control.
    pub const IMAGE_SIZE: &str = "Størrelse";
    /// Image: square.
    pub const IMAGE_SIZE_SQUARE: &str = "Kvadratisk";
    /// Image: landscape.
    pub const IMAGE_SIZE_LANDSCAPE: &str = "Liggende";
    /// Image: portrait.
    pub const IMAGE_SIZE_PORTRAIT: &str = "Stående";
    /// Image: label for the quality control.
    pub const IMAGE_QUALITY: &str = "Kvalitet";
    /// Image quality: quickest.
    pub const QUALITY_LOW: &str = "Lav (raskest)";
    /// Image quality: balanced.
    pub const QUALITY_MEDIUM: &str = "Middels";
    /// Image quality: slowest and most detailed.
    pub const QUALITY_HIGH: &str = "Høy (tar lengst tid)";

    /// Audio: label for the voice control.
    pub const AUDIO_VOICE: &str = "Stemme";
    /// Audio: voice descriptions. The names are the API values and are not
    /// translated; the parenthetical is what tells a user them apart.
    pub const VOICE_NOVA: &str = "Nova – lys og vennlig";
    /// Voice: alloy.
    pub const VOICE_ALLOY: &str = "Alloy – nøytral";
    /// Voice: echo.
    pub const VOICE_ECHO: &str = "Echo – rolig";
    /// Voice: fable.
    pub const VOICE_FABLE: &str = "Fable – fortellende";
    /// Voice: onyx.
    pub const VOICE_ONYX: &str = "Onyx – dyp";
    /// Voice: shimmer.
    pub const VOICE_SHIMMER: &str = "Shimmer – varm";

    /// Video: label for the length control.
    pub const VIDEO_LENGTH: &str = "Lengde";
    /// Video: four seconds.
    pub const VIDEO_4S: &str = "4 sekunder";
    /// Video: eight seconds.
    pub const VIDEO_8S: &str = "8 sekunder";
    /// Video: twelve seconds.
    pub const VIDEO_12S: &str = "12 sekunder";
    /// Video: label for the frame format control.
    pub const VIDEO_FORMAT: &str = "Format";
    /// Video: landscape frame.
    pub const VIDEO_LANDSCAPE: &str = "Liggende";
    /// Video: portrait frame.
    pub const VIDEO_PORTRAIT: &str = "Stående";
    /// Video: square frame.
    pub const VIDEO_SQUARE: &str = "Kvadratisk";

    /// Skip link shown to keyboard users at the top of every page.
    pub const SKIP_TO_CONTENT: &str = "Hopp til hovedinnhold";

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
    /// Heading of the result card.
    pub const RESULT_HEADING: &str = "Resultat";
    /// Heading of the card while the job is still running.
    pub const RESULT_PENDING_HEADING: &str = "Genererer";
    /// Heading of the card when the job failed.
    pub const RESULT_FAILED_HEADING: &str = "Genereringen feilet";
    /// Status text while the job waits for a free worker.
    pub const STATUS_QUEUED: &str = "I kø …";
    /// Status text while the job is being generated.
    pub const STATUS_RUNNING: &str = "Genererer …";
    /// Reassurance that leaving the page does not cancel the job.
    pub const PENDING_SAFE_TO_LEAVE: &str =
        "Du kan trygt navigere vekk eller oppdatere siden. Du får beskjed når den er ferdig.";
    /// Label in front of the prompt on the result card.
    pub const RESULT_PROMPT_LABEL: &str = "Beskrivelse:";
    /// Shown in place of a preview while generation is still mocked.
    pub const RESULT_MOCK_NOTICE: &str = "Forhåndsvisning kommer når genereringen kobles mot Azure AI Foundry. \
         Grensesnittet er ferdig – dette er en test av flyten.";

    /// Title of the history page.
    pub const PAGE_HISTORY_TITLE: &str = "Min historikk";
    /// Ingress of the history page.
    pub const PAGE_HISTORY_LEAD: &str = "Her finner du alt du har generert, med nyeste først.";
    /// Title of the archive page.
    pub const PAGE_ARCHIVE_TITLE: &str = "Ferdig generert innhold";
    /// Ingress of the archive page.
    pub const PAGE_ARCHIVE_LEAD: &str = "Arkiv over generert innhold. Du kan velge mellom dine egne og alle, \
         sortere etter dato og filtrere på bilde, lyd og video.";
    /// Headline when the session is missing or expired.
    pub const ERROR_HEADING_SIGN_IN: &str = "Du må logge inn";
    /// Headline when the user lacks the required app role.
    pub const ERROR_HEADING_NO_ACCESS: &str = "Ingen tilgang";
    /// Headline when nothing matched.
    pub const ERROR_HEADING_NOT_FOUND: &str = "Fant ikke siden";
    /// Headline when the user is rate limited.
    pub const ERROR_HEADING_TOO_MUCH: &str = "Vent litt";
    /// Headline for anything else.
    pub const ERROR_HEADING_SOMETHING_WRONG: &str = "Noe gikk galt";
    /// Offer to start the sign-in over.
    pub const TRY_SIGN_IN_AGAIN: &str = "Prøv å logge inn på nytt";
    /// Label in front of the correlation id on the error page.
    pub const SUPPORT_REFERENCE: &str = "Oppgi denne referansen ved henvendelse:";

    /// Accessible label for the filter group.
    pub const FILTERS_LABEL: &str = "Filtrer og sorter";
    /// Label above the own/all toggle.
    pub const FILTER_SCOPE: &str = "Vis";
    /// Scope: only the viewer's own work.
    pub const FILTER_OWN: &str = "Egne";
    /// Scope: everyone's work.
    pub const FILTER_ALL: &str = "Alle";
    /// Label above the media type filter.
    pub const FILTER_TYPE: &str = "Type";
    /// Media filter: no restriction.
    pub const FILTER_ALL_TYPES: &str = "Alle";
    /// Label above the sort control.
    pub const FILTER_SORT: &str = "Sorter";
    /// Sort: newest first.
    pub const SORT_NEWEST: &str = "Nyeste";
    /// Sort: oldest first.
    pub const SORT_OLDEST: &str = "Eldste";
    /// Shown when a filter matched nothing.
    pub const EMPTY_NOTHING_FOUND: &str =
        "Ingenting å vise her ennå. Prøv et annet filter, eller generer noe nytt.";
    /// Shown in place of a preview when the job produced no file.
    pub const NO_PREVIEW: &str = "Ingen forhåndsvisning";
    /// Marker on a card the owner has hidden from the shared archive.
    pub const HIDDEN_MARKER: &str = "skjult for andre";
    /// Action: hide this from colleagues.
    pub const HIDE_FROM_OTHERS: &str = "Skjul for andre";
    /// Action: show it again.
    pub const SHOW_TO_OTHERS: &str = "Vis for andre";
    /// Action: delete permanently.
    pub const DELETE: &str = "Slett";
    /// Confirmation before an irreversible delete.
    pub const CONFIRM_DELETE: &str =
        "Slette denne for godt? Både beskrivelsen og filen fjernes, og det kan ikke angres.";
    /// Toast after hiding.
    pub const HIDDEN_DONE: &str = "Skjult for andre";
    /// Toast after unhiding.
    pub const SHOWN_DONE: &str = "Synlig for andre igjen";
    /// Toast after deleting.
    pub const DELETED_DONE: &str = "Slettet";

    /// Attribution for the viewer's own work.
    pub const BY_YOU: &str = "Generert av deg";
    /// Accessible label for the pagination controls.
    pub const PAGINATION_LABEL: &str = "Sidenavigasjon";
    /// Link to the previous page.
    pub const PREVIOUS_PAGE: &str = "← Forrige";
    /// Link to the next page.
    pub const NEXT_PAGE: &str = "Neste →";
    /// Shown when the hourly limit is nearly used up.
    pub const RATE_LIMIT_REMAINING_LABEL: &str = "igjen denne timen";

    /// Empty state on pages that are not built yet.
    pub const EMPTY_COMING_IN_PHASE_6: &str = "Denne siden fylles med innhold senere.";
    /// Link back to the generator from an empty page.
    pub const BACK_TO_GENERATOR: &str = "Tilbake til generatoren";

    /// Confirmation shown after copying a link to the clipboard.
    pub const COPIED: &str = "Lenken er kopiert";

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
    /// Generic validation message, used when only a code is available.
    /// Shown when the global job queue is saturated.
    /// Shown when an asynchronous generation never finished in time.
    pub const ERR_GENERATION_TIMEOUT: &str = "Genereringen tok for lang tid og ble avbrutt. Prøv igjen, gjerne med en kortere beskrivelse.";

    pub const ERR_QUEUE_FULL: &str =
        "Det er mye som genereres akkurat nå. Prøv igjen om et par minutter.";
    /// Shown for a job left unfinished when the server restarted.
    pub const ERR_JOB_INTERRUPTED: &str =
        "Genereringen ble avbrutt fordi tjenesten startet på nytt. Prøv igjen.";

    pub const ERR_VALIDATION: &str = "Det du sendte inn kunne ikke brukes.";

    /// Shown when the sign-in flow failed for any reason we will not detail.
    pub const ERR_LOGIN_FAILED: &str = "Innloggingen feilet. Prøv å logge inn på nytt.";
    /// Shown when the prompt is empty.
    pub const ERR_PROMPT_EMPTY: &str = "Du må beskrive hva du vil lage.";
    /// Shown when the prompt contains disallowed control characters.
    pub const ERR_PROMPT_CONTROL_CHARS: &str = "Beskrivelsen inneholder ugyldige tegn.";
    /// Shown when the submitted media type is not one we know.
    pub const ERR_UNKNOWN_MEDIA_TYPE: &str = "Ukjent medietype.";

    /// Unit suffix for a duration in seconds.
    pub const SECONDS_SUFFIX: &str = "s";

    /// Returns the "page X of Y" line under a listing.
    pub fn page_of(page: i64, pages: i64) -> String {
        format!("Side {page} av {pages}")
    }

    /// Returns the remaining-generations hint shown under the prompt.
    pub fn generations_remaining(remaining: u32) -> String {
        format!("{remaining} genereringer {RATE_LIMIT_REMAINING_LABEL}")
    }

    /// Returns the message shown when the prompt exceeds `max` characters.
    pub fn err_prompt_too_long(max: usize) -> String {
        format!("Beskrivelsen kan være maks {max} tegn.")
    }
}
