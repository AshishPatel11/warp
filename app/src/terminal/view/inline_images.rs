use warpui::AppContext;

use super::TerminalView;
use crate::terminal::TerminalModel;
use crate::terminal::alt_screen::inline_images::InlineThumbnail;

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use std::sync::LazyLock;

        use warp_core::ui::theme::WarpTheme;
        use warpui::assets::asset_cache::AssetSource;
        use warpui::elements::{
            Border, CacheOption, Container, CornerRadius, Image, Radius,
        };
        use warpui::geometry::vector::vec2f;
        use warpui::{Element, SingletonEntity};

        use crate::appearance::Appearance;
        use crate::terminal::alt_screen::inline_images::{
            ResolvedImage, find_image_path_mentions, is_blank, place_thumbnails,
        };
        use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;
        use crate::terminal::model::block::BlockMetadata;
        use crate::terminal::model::grid::Dimensions;
    }
}

/// Inline image thumbnails are on unless this environment variable is set to `0`.
#[cfg(feature = "local_fs")]
static INLINE_IMAGE_PREVIEWS_ENABLED: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("WARP_INLINE_IMAGE_PREVIEWS").map_or(true, |value| value != "0")
});

#[cfg(feature = "local_fs")]
const THUMBNAIL_PADDING: f32 = 2.;
#[cfg(feature = "local_fs")]
const THUMBNAIL_CORNER_RADIUS: f32 = 4.;

impl TerminalView {
    /// Builds thumbnails for image files mentioned in a CLI agent's full-screen UI.
    #[cfg(feature = "local_fs")]
    pub(super) fn inline_image_thumbnails(
        &self,
        model: &TerminalModel,
        app: &AppContext,
    ) -> Vec<InlineThumbnail> {
        if !*INLINE_IMAGE_PREVIEWS_ENABLED
            || CLIAgentSessionsModel::as_ref(app)
                .session(self.view_id)
                .is_none()
        {
            return Vec::new();
        }
        // Not `pwd_if_local`: it locks the terminal model, which the caller already holds.
        if model.is_shared_session_viewer() || model.is_conversation_transcript_viewer() {
            return Vec::new();
        }
        let Some(session) = self
            .active_block_session_id()
            .and_then(|session_id| self.sessions.as_ref(app).get(session_id))
            .filter(|session| session.is_local())
        else {
            return Vec::new();
        };
        let shell_launch_data = session.launch_data().cloned();
        let Some(working_directory) = self
            .active_block_metadata
            .as_ref()
            .and_then(BlockMetadata::current_working_directory)
            .and_then(|cwd| shell_launch_data.as_ref()?.maybe_convert_absolute_path(cwd))
            .map(|path| path.to_string_lossy().into_owned())
        else {
            return Vec::new();
        };

        let grid = model.alt_screen().grid_handler();
        let grid_size = (grid.visible_rows(), grid.columns());
        let images: Vec<ResolvedImage> = find_image_path_mentions(grid, 0..grid_size.0)
            .into_iter()
            .filter_map(|mention| {
                self.inline_image_paths.resolve(
                    mention,
                    &working_directory,
                    shell_launch_data.as_ref(),
                )
            })
            .collect();
        if images.is_empty() {
            return Vec::new();
        }

        let cell_size = vec2f(
            self.size_info.cell_width_px().as_f32(),
            self.size_info.cell_height_px().as_f32(),
        );
        let theme = Appearance::as_ref(app).theme();
        place_thumbnails(&images, grid_size, cell_size, |row, columns| {
            is_blank(grid, row, columns)
        })
        .into_iter()
        .map(|placement| {
            let image = &images[placement.image_index];
            InlineThumbnail {
                element: render_thumbnail(image.asset_source(), theme),
                path: image.path.clone(),
                placement,
            }
        })
        .collect()
    }

    #[cfg(not(feature = "local_fs"))]
    pub(super) fn inline_image_thumbnails(
        &self,
        _model: &TerminalModel,
        _app: &AppContext,
    ) -> Vec<InlineThumbnail> {
        Vec::new()
    }
}

#[cfg(feature = "local_fs")]
fn render_thumbnail(source: AssetSource, theme: &WarpTheme) -> Box<dyn Element> {
    Container::new(Image::new(source, CacheOption::BySize).contain().finish())
        .with_uniform_padding(THUMBNAIL_PADDING)
        .with_background(theme.surface_2())
        .with_border(Border::all(1.).with_border_fill(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
            THUMBNAIL_CORNER_RADIUS,
        )))
        .finish()
}
