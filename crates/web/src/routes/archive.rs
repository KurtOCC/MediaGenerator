//! The history and archive pages.
//!
//! Both render the same grid from the same query; they differ only in the
//! filter they start from. `/historikk` is the user's own work, failures
//! included, so they can see what went wrong. `/eksempler` is the shared
//! archive of finished media, with the own/all, media type and sort controls.
//!
//! Every control is an ordinary `<form method="get">`, so the filters work
//! without JavaScript, survive a reload, and can be bookmarked and shared.

use askama::Template;
use axum::{
    Extension, Router,
    extract::{Query, State},
    routing::get,
};
use mediagenerator_auth::SessionUser;
use mediagenerator_domain::{JobListItem, ListFilter, MediaType, i18n::nb};
use mediagenerator_storage::jobs;
use serde::Deserialize;
use tower_sessions::Session;
#[cfg(test)]
use uuid::Uuid;

use crate::{
    error::AppError, identity::local_user_id, middleware::csrf, render::Page, state::AppState,
};

/// Rows per page.
///
/// Divides evenly by three, which is the desktop column count, so the last row
/// of a full page is never ragged.
const PAGE_SIZE: i64 = 24;

/// Returns the archive routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/historikk", get(history))
        .route("/eksempler", get(archive))
}

/// Query parameters shared by both pages.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ArchiveQuery {
    /// `egne` or `alle`. Ignored on the history page, which is always own.
    #[serde(default)]
    eier: Option<String>,
    /// `bilde`, `lyd`, `video`, or absent for all.
    ///
    /// `type` is a Rust keyword, hence the trailing underscore and the rename:
    /// the URL says `type`, which is what reads well in a shared link.
    #[serde(default, rename = "type")]
    type_: Option<String>,
    /// `nyest` or `eldst`.
    #[serde(default)]
    sortering: Option<String>,
    /// One-based page number.
    #[serde(default)]
    side: Option<i64>,
}

impl ArchiveQuery {
    /// Returns `true` when the listing should be limited to the viewer.
    fn own_only(&self) -> bool {
        // Own is the default: a shared archive that opens on everyone's work
        // is a surprise, and the toggle is right there.
        self.eier.as_deref() != Some("alle")
    }

    /// Returns the media type filter, if one was asked for.
    ///
    /// The values are the Norwegian words the buttons use, so the URL reads as
    /// the page does. An unrecognised value means no filter rather than an
    /// error: a hand-edited URL should degrade, not break.
    fn media_type(&self) -> Option<MediaType> {
        match self.type_.as_deref() {
            Some("bilde") => Some(MediaType::Image),
            Some("lyd") => Some(MediaType::Audio),
            Some("video") => Some(MediaType::Video),
            _ => None,
        }
    }

    /// Returns `true` when the newest should come first.
    fn newest_first(&self) -> bool {
        self.sortering.as_deref() != Some("eldst")
    }

    /// Returns the zero-based page index, clamped to something sensible.
    fn page(&self) -> i64 {
        self.side.unwrap_or(1).max(1) - 1
    }

    /// Rebuilds the query string with one parameter replaced.
    ///
    /// Used for the filter links, so changing the media type keeps the sort and
    /// the scope rather than resetting them.
    fn with(&self, key: &str, value: &str) -> String {
        let mut parts: Vec<(&str, String)> = Vec::new();

        let eier = if key == "eier" {
            value.to_owned()
        } else if self.own_only() {
            "egne".to_owned()
        } else {
            "alle".to_owned()
        };
        parts.push(("eier", eier));

        let type_ = if key == "type" {
            value.to_owned()
        } else {
            self.type_.clone().unwrap_or_else(|| "alle".to_owned())
        };
        if type_ != "alle" {
            parts.push(("type", type_));
        }

        let sortering = if key == "sortering" {
            value.to_owned()
        } else if self.newest_first() {
            "nyest".to_owned()
        } else {
            "eldst".to_owned()
        };
        parts.push(("sortering", sortering));

        // Changing a filter always returns to the first page; staying on page
        // four of a list that just became shorter shows nothing.
        if key == "side" {
            parts.push(("side", value.to_owned()));
        }

        let query = parts
            .iter()
            .map(|(name, value)| format!("{name}={}", encode(value)))
            .collect::<Vec<_>>()
            .join("&");

        format!("?{query}")
    }
}

/// Percent-encodes a query parameter value.
fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// One entry in the rendered grid.
pub struct Entry {
    /// Job identifier.
    pub id: String,
    /// `image`, `audio` or `video`, for the icon.
    pub media_type: &'static str,
    /// Norwegian label for the media type.
    pub media_label: &'static str,
    /// The prompt.
    pub prompt: String,
    /// When it was generated, as `dd.mm.yyyy`.
    pub created: String,
    /// Who generated it.
    pub owner_name: String,
    /// True when the viewer owns it.
    pub own: bool,
    /// Link to the asset, empty when there is none.
    pub asset_url: String,
    /// True when the owner has hidden this from the shared archive.
    pub hidden: bool,
    /// True when the job failed.
    pub failed: bool,
    /// Norwegian failure message, when it failed.
    pub error_message: String,
}

impl Entry {
    /// Builds a grid entry from a listing row.
    fn from_item(item: JobListItem) -> Self {
        let failed = matches!(item.status, mediagenerator_domain::JobStatus::Failed { .. });

        Self {
            id: item.id.to_string(),
            media_type: item.media_type.as_str(),
            media_label: item.media_type.label(),
            prompt: item.prompt,
            created: format!(
                "{:02}.{:02}.{}",
                item.created_at.day(),
                u8::from(item.created_at.month()),
                item.created_at.year()
            ),
            owner_name: item.owner_name,
            own: item.owned_by_viewer,
            asset_url: item
                .asset_id
                .map(|id| format!("/api/assets/{id}"))
                .unwrap_or_default(),
            hidden: item.hidden,
            failed,
            error_message: item
                .status
                .error_message()
                .unwrap_or(nb::ERR_INTERNAL)
                .to_owned(),
        }
    }
}

/// One page of a listing.
#[derive(Template)]
#[template(path = "arkiv.html")]
pub struct ArchiveTemplate {
    csrf_token: String,
    initials: String,
    display_name: String,
    /// Page heading.
    title: &'static str,
    /// Ingress under the heading.
    lead: &'static str,
    /// True on `/eksempler`, which is the page with the own/all toggle.
    show_scope: bool,
    /// The rows on this page.
    entries: Vec<Entry>,
    /// True when the viewer's own work is being shown.
    own_only: bool,
    /// Which media filter is active: `alle`, `bilde`, `lyd` or `video`.
    active_type: String,
    /// True when sorted newest first.
    newest_first: bool,
    /// One-based page number.
    page: i64,
    /// Total number of pages, at least one.
    pages: i64,
    /// Link to the previous page, empty on the first.
    previous_url: String,
    /// Link to the next page, empty on the last.
    next_url: String,
    /// Links for the filter controls, prebuilt so the template holds no logic.
    url_own: String,
    url_all: String,
    url_type_all: String,
    url_type_image: String,
    url_type_audio: String,
    url_type_video: String,
    url_newest: String,
    url_oldest: String,
}

/// Renders the signed-in user's own history.
async fn history(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    Query(query): Query<ArchiveQuery>,
) -> Result<Page<ArchiveTemplate>, AppError> {
    // Failures included: the point of a history is to see what happened,
    // including the generation that was refused.
    render(
        &state,
        &session,
        &user,
        &query,
        nb::PAGE_HISTORY_TITLE,
        nb::PAGE_HISTORY_LEAD,
        false,
        true,
        false,
    )
    .await
}

/// Renders the shared archive of finished media.
async fn archive(
    State(state): State<AppState>,
    session: Session,
    Extension(user): Extension<SessionUser>,
    Query(query): Query<ArchiveQuery>,
) -> Result<Page<ArchiveTemplate>, AppError> {
    render(
        &state,
        &session,
        &user,
        &query,
        nb::PAGE_ARCHIVE_TITLE,
        nb::PAGE_ARCHIVE_LEAD,
        true,
        query.own_only(),
        true,
    )
    .await
}

/// Runs the query and builds the page.
#[allow(clippy::too_many_arguments)]
async fn render(
    state: &AppState,
    session: &Session,
    user: &SessionUser,
    query: &ArchiveQuery,
    title: &'static str,
    lead: &'static str,
    show_scope: bool,
    own_only: bool,
    only_succeeded: bool,
) -> Result<Page<ArchiveTemplate>, AppError> {
    let viewer = local_user_id(&state.db, session, user).await?;

    let filter = ListFilter {
        user_id: own_only.then_some(viewer),
        media_type: query.media_type(),
        only_succeeded,
        // Someone else's hidden work is not shown; your own always is.
        exclude_hidden: !own_only,
        newest_first: query.newest_first(),
        limit: PAGE_SIZE,
        offset: query.page() * PAGE_SIZE,
    };

    let total = jobs::count(&state.db, &filter)
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))?;

    let items = jobs::list(&state.db, &filter, viewer)
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))?;

    let pages = total.div_euclid(PAGE_SIZE) + i64::from(total.rem_euclid(PAGE_SIZE) > 0);
    let pages = pages.max(1);
    let page = query.page() + 1;

    Ok(Page(ArchiveTemplate {
        csrf_token: csrf_token(session).await?,
        initials: user.initials(),
        display_name: user.display_name.clone(),
        title,
        lead,
        show_scope,
        entries: items.into_iter().map(Entry::from_item).collect(),
        own_only,
        active_type: query.type_.clone().unwrap_or_else(|| "alle".to_owned()),
        newest_first: query.newest_first(),
        page,
        pages,
        previous_url: if page > 1 {
            query.with("side", &(page - 1).to_string())
        } else {
            String::new()
        },
        next_url: if page < pages {
            query.with("side", &(page + 1).to_string())
        } else {
            String::new()
        },
        url_own: query.with("eier", "egne"),
        url_all: query.with("eier", "alle"),
        url_type_all: query.with("type", "alle"),
        url_type_image: query.with("type", "bilde"),
        url_type_audio: query.with("type", "lyd"),
        url_type_video: query.with("type", "video"),
        url_newest: query.with("sortering", "nyest"),
        url_oldest: query.with("sortering", "eldst"),
    }))
}

/// Reads or creates this session's CSRF token.
async fn csrf_token(session: &Session) -> Result<String, AppError> {
    csrf::token(session)
        .await
        .map_err(|error| AppError::Internal(anyhow::Error::new(error)))
}

/// Returns the identifier used by the tests to build a viewer.
#[cfg(test)]
fn test_viewer() -> Uuid {
    Uuid::nil()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(raw: &str) -> ArchiveQuery {
        serde_urlencoded::from_str(raw).expect("the query should parse")
    }

    #[test]
    fn the_archive_template_renders_its_filters_and_grid() {
        let entries = vec![Entry::from_item(JobListItem {
            id: Uuid::nil(),
            media_type: MediaType::Audio,
            prompt: "En vennlig norsk stemme".to_owned(),
            status: mediagenerator_domain::JobStatus::Succeeded,
            created_at: time::OffsetDateTime::now_utc(),
            asset_id: Some(Uuid::nil()),
            owner_name: "Hans Kristiansen".to_owned(),
            owned_by_viewer: false,
            hidden: false,
        })];

        let current = query("eier=alle&type=lyd");
        let html = ArchiveTemplate {
            csrf_token: "t".to_owned(),
            initials: "HK".to_owned(),
            display_name: "Hans Kristiansen".to_owned(),
            title: nb::PAGE_ARCHIVE_TITLE,
            lead: nb::PAGE_ARCHIVE_LEAD,
            show_scope: true,
            entries,
            own_only: false,
            active_type: "lyd".to_owned(),
            newest_first: true,
            page: 1,
            pages: 3,
            previous_url: String::new(),
            next_url: current.with("side", "2"),
            url_own: current.with("eier", "egne"),
            url_all: current.with("eier", "alle"),
            url_type_all: current.with("type", "alle"),
            url_type_image: current.with("type", "bilde"),
            url_type_audio: current.with("type", "lyd"),
            url_type_video: current.with("type", "video"),
            url_newest: current.with("sortering", "nyest"),
            url_oldest: current.with("sortering", "eldst"),
        }
        .render()
        .expect("the archive should render");

        // All three filter groups are present.
        assert!(html.contains(nb::FILTER_OWN));
        assert!(html.contains(nb::FILTER_ALL));
        assert!(html.contains(nb::SORT_NEWEST));
        assert!(html.contains(nb::SORT_OLDEST));
        assert!(html.contains(nb::MEDIA_VIDEO));
        // The active filter is marked for assistive technology, not just visually.
        assert!(html.contains("aria-current=\"true\""));
        // The entry, attributed to whoever made it.
        assert!(html.contains("En vennlig norsk stemme"));
        assert!(html.contains("Hans Kristiansen"));
        assert!(!html.contains(nb::BY_YOU));
        // Pagination, with the previous link disabled on page one.
        assert!(html.contains("Side 1 av 3"));
        assert!(html.contains("sidelenke-av"));
    }

    #[test]
    fn an_empty_listing_says_so_instead_of_showing_a_bare_grid() {
        let current = query("");
        let html = ArchiveTemplate {
            csrf_token: "t".to_owned(),
            initials: "HK".to_owned(),
            display_name: "Hans Kristiansen".to_owned(),
            title: nb::PAGE_HISTORY_TITLE,
            lead: nb::PAGE_HISTORY_LEAD,
            show_scope: false,
            entries: Vec::new(),
            own_only: true,
            active_type: "alle".to_owned(),
            newest_first: true,
            page: 1,
            pages: 1,
            previous_url: String::new(),
            next_url: String::new(),
            url_own: current.with("eier", "egne"),
            url_all: current.with("eier", "alle"),
            url_type_all: current.with("type", "alle"),
            url_type_image: current.with("type", "bilde"),
            url_type_audio: current.with("type", "lyd"),
            url_type_video: current.with("type", "video"),
            url_newest: current.with("sortering", "nyest"),
            url_oldest: current.with("sortering", "eldst"),
        }
        .render()
        .expect("the archive should render");

        assert!(html.contains(nb::EMPTY_NOTHING_FOUND));
        // The scope toggle belongs to the archive, not to the history page.
        assert!(!html.contains(nb::FILTER_SCOPE));
        // One page means no pagination at all.
        assert!(!html.contains(nb::PAGINATION_LABEL));
    }

    #[test]
    fn own_is_the_default_scope() {
        assert!(query("").own_only());
        assert!(query("eier=egne").own_only());
        assert!(!query("eier=alle").own_only());
        // A hand-edited value degrades to the safe default.
        assert!(query("eier=tull").own_only());
    }

    #[test]
    fn the_media_filter_uses_the_norwegian_words() {
        assert_eq!(query("type=bilde").media_type(), Some(MediaType::Image));
        assert_eq!(query("type=lyd").media_type(), Some(MediaType::Audio));
        assert_eq!(query("type=video").media_type(), Some(MediaType::Video));
        assert_eq!(query("type=alle").media_type(), None);
        assert_eq!(query("").media_type(), None);
    }

    #[test]
    fn newest_first_is_the_default_sort() {
        assert!(query("").newest_first());
        assert!(query("sortering=nyest").newest_first());
        assert!(!query("sortering=eldst").newest_first());
    }

    #[test]
    fn pages_are_one_based_on_the_way_in_and_zero_based_on_the_way_out() {
        assert_eq!(query("").page(), 0);
        assert_eq!(query("side=1").page(), 0);
        assert_eq!(query("side=3").page(), 2);
        // A nonsensical page number must not produce a negative offset.
        assert_eq!(query("side=0").page(), 0);
        assert_eq!(query("side=-5").page(), 0);
    }

    #[test]
    fn changing_one_filter_keeps_the_others() {
        let current = query("eier=alle&type=lyd&sortering=eldst");

        let switched = current.with("type", "video");
        assert!(switched.contains("eier=alle"));
        assert!(switched.contains("type=video"));
        assert!(switched.contains("sortering=eldst"));
    }

    #[test]
    fn changing_a_filter_returns_to_the_first_page() {
        // Page four of a list that just got shorter would show nothing.
        let current = query("side=4&type=bilde");
        assert!(!current.with("type", "lyd").contains("side="));
        // Paging itself of course keeps the page.
        assert!(current.with("side", "5").contains("side=5"));
    }

    #[test]
    fn the_all_filter_is_left_out_of_the_url() {
        // "everything" is the default, so it does not need saying.
        assert!(!query("").with("type", "alle").contains("type="));
    }

    #[test]
    fn a_grid_entry_renders_its_date_norwegian_style() {
        let item = JobListItem {
            id: Uuid::nil(),
            media_type: MediaType::Image,
            prompt: "Solnedgang".to_owned(),
            status: mediagenerator_domain::JobStatus::Succeeded,
            created_at: time::OffsetDateTime::from_unix_timestamp(1_789_000_000)
                .expect("valid timestamp"),
            asset_id: Some(Uuid::nil()),
            owner_name: "Hans Kristiansen".to_owned(),
            owned_by_viewer: true,
            hidden: false,
        };

        let entry = Entry::from_item(item);
        assert_eq!(entry.created, "10.09.2026");
        assert_eq!(entry.asset_url, format!("/api/assets/{}", Uuid::nil()));
        assert!(!entry.failed);
        assert_eq!(test_viewer(), Uuid::nil());
    }

    #[test]
    fn a_failed_job_keeps_its_message_and_has_no_asset() {
        let item = JobListItem {
            id: Uuid::nil(),
            media_type: MediaType::Video,
            prompt: "noe".to_owned(),
            status: mediagenerator_domain::JobStatus::Failed {
                code: mediagenerator_domain::ErrorCode::ContentFilter,
                message: nb::ERR_CONTENT_FILTER.to_owned(),
            },
            created_at: time::OffsetDateTime::now_utc(),
            asset_id: None,
            owner_name: "Hans Kristiansen".to_owned(),
            owned_by_viewer: true,
            hidden: false,
        };

        let entry = Entry::from_item(item);
        assert!(entry.failed);
        assert_eq!(entry.error_message, nb::ERR_CONTENT_FILTER);
        assert!(entry.asset_url.is_empty());
    }
}
