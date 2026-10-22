use warpui::AppContext;

use super::TerminalView;
use crate::terminal::TerminalModel;
use crate::terminal::alt_screen::inline_images::InlineThumbnailFinder;

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use std::path::PathBuf;
        use std::sync::LazyLock;

        use warpui::SingletonEntity;
        use warpui::geometry::vector::Vector2F;

        use crate::terminal::model::grid::grid_handler::GridHandler;
        use crate::terminal::alt_screen::inline_images::{
            InlineThumbnail, ResolvedImage, find_image_path_mentions, is_blank, place_thumbnails,
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

impl TerminalView {
    /// Returns how to find image thumbnails in a CLI agent's full-screen UI, or `None` if this
    /// pane shouldn't show any.
    #[cfg(feature = "local_fs")]
    pub(super) fn inline_thumbnail_finder(
        &self,
        model: &TerminalModel,
        app: &AppContext,
    ) -> Option<InlineThumbnailFinder> {
        if !*INLINE_IMAGE_PREVIEWS_ENABLED
            || CLIAgentSessionsModel::as_ref(app)
                .session(self.view_id)
                .is_none()
            || model.is_shared_session_viewer()
            || model.is_conversation_transcript_viewer()
        {
            return None;
        }
        // Not `pwd_if_local`: it locks the terminal model, which the caller already holds.
        let session = self
            .active_block_session_id()
            .and_then(|session_id| self.sessions.as_ref(app).get(session_id));
        if session.as_ref().is_some_and(|session| !session.is_local()) {
            return None;
        }
        let shell_launch_data = session.and_then(|session| session.launch_data().cloned());
        // Only relative paths need the working directory, so absolute ones still resolve without it.
        let working_directory = self
            .active_block_metadata
            .as_ref()
            .and_then(BlockMetadata::current_working_directory)
            .map(String::from)
            .or_else(|| model.block_list().active_block().pwd().cloned())
            .map(|cwd| {
                shell_launch_data
                    .as_ref()
                    .and_then(|data| data.maybe_convert_absolute_path(&cwd))
                    .unwrap_or_else(|| PathBuf::from(cwd))
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default();
        let path_cache = self.inline_image_paths.clone();

        Some(Box::new(move |grid: &GridHandler, cell_size: Vector2F| {
            let grid_size = (grid.visible_rows(), grid.columns());
            let images: Vec<ResolvedImage> = find_image_path_mentions(grid, 0..grid_size.0)
                .into_iter()
                .filter_map(|mention| {
                    path_cache.resolve(mention, &working_directory, shell_launch_data.as_ref())
                })
                .collect();
            place_thumbnails(&images, grid_size, cell_size, |row, columns| {
                is_blank(grid, row, columns)
            })
            .into_iter()
            .map(|placement| {
                let image = &images[placement.image_index];
                InlineThumbnail {
                    source: image.asset_source(),
                    path: image.path.clone(),
                    placement,
                }
            })
            .collect()
        }))
    }

    #[cfg(not(feature = "local_fs"))]
    pub(super) fn inline_thumbnail_finder(
        &self,
        _model: &TerminalModel,
        _app: &AppContext,
    ) -> Option<InlineThumbnailFinder> {
        None
    }
}
