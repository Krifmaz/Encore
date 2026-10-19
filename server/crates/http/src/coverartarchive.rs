//! Cover Art Archive: cover images of MusicBrainz releases and release groups.
use crate::Ctx;
use reqwest::RequestBuilder;

pub const BASE: &str = "https://coverartarchive.org";

/// Every image the archive has of a `kind` ("release", "release-group") with this id, as JSON.
pub fn images(st: &impl Ctx, kind: &str, id: &str) -> RequestBuilder {
    st.http().get(format!("{BASE}/{kind}/{id}"))
}

/// The front cover of a release group at `size` pixels (250, 500, 1200).
pub fn cover_rg(id: &str, size: u32) -> String {
    format!("{BASE}/release-group/{id}/front-{size}")
}

/// One image (`image_id`, as ListenBrainz names it) of a release at `size` pixels.
pub fn image(release: &str, image_id: &str, size: u32) -> String {
    format!("{BASE}/release/{release}/{image_id}-{size}.jpg")
}

/// The front cover of one release at `size` pixels.
pub fn cover_release(id: &str, size: u32) -> String {
    format!("{BASE}/release/{id}/front-{size}")
}
