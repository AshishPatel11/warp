use warpui::AppContext;

use super::TerminalView;
use crate::terminal::TerminalModel;
use crate::terminal::alt_screen::inline_images::InlineThumbnailFinder;

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use std::cell::RefCell;
        use std::fs::OpenOptions;
        use std::io::Write as _;
        use std::path::PathBuf;
        use std::sync::LazyLock;
        use std::time::{SystemTime, UNIX_EPOCH};

        use warpui::SingletonEntity;
        use warpui::geometry::vector::Vector2F;

        use crate::terminal::alt_screen::inline_images::{
            InlineThumbnail, ResolvedImage, debug_describe_row, debug_rows_containing,
            find_image_path_mentions, is_blank, place_thumbnails,
        };
        use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;
        use crate::terminal::model::block::BlockMetadata;
        use crate::terminal::model::grid::Dimensions;
        use crate::terminal::model::grid::grid_handler::GridHandler;
    }
}

/// Inline image thumbnails are on unless this environment variable is set to `0`.
#[cfg(feature = "local_fs")]
static INLINE_IMAGE_PREVIEWS_ENABLED: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("WARP_INLINE_IMAGE_PREVIEWS").map_or(true, |value| value != "0")
});

impl TerminalView {
    /// Returns how to find image thumbnails in a CLI agent's full-screen UI, or `None` if this
    /// pane can't show any.
    #[cfg(feature = "local_fs")]
    pub(super) fn inline_thumbnail_finder(
        &self,
        model: &TerminalModel,
        app: &AppContext,
    ) -> Option<InlineThumbnailFinder> {
        if !*INLINE_IMAGE_PREVIEWS_ENABLED
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
        let view_id = self.view_id;

        Some(Box::new(
            move |grid: &GridHandler, cell_size: Vector2F, app: &AppContext| {
                // Checked while painting because Warp registers the CLI agent session a moment
                // after the agent starts, which can be after the last render.
                let has_agent_session = CLIAgentSessionsModel::as_ref(app)
                    .session(view_id)
                    .is_some();
                let grid_size = (grid.visible_rows(), grid.columns());
                let mentions = if has_agent_session {
                    find_image_path_mentions(grid, 0..grid_size.0)
                } else {
                    Vec::new()
                };
                let mention_texts: Vec<String> = mentions
                    .iter()
                    .take(6)
                    .map(|mention| {
                        format!("{}@{}:{:?}", mention.text, mention.row, mention.columns)
                    })
                    .collect();
                let images: Vec<ResolvedImage> = mentions
                    .into_iter()
                    .filter_map(|mention| {
                        path_cache.resolve(mention, &working_directory, shell_launch_data.as_ref())
                    })
                    .collect();
                let placements = place_thumbnails(&images, grid_size, cell_size, |row, columns| {
                    is_blank(grid, row, columns)
                });

                let first_image_rows: Vec<String> = images
                    .first()
                    .map(|image| {
                        let row_end = (image.mention.row + 4).min(grid_size.0);
                        (image.mention.row..row_end)
                            .map(|row| {
                                debug_describe_row(
                                    grid,
                                    row,
                                    grid_size.1.saturating_sub(20)..grid_size.1,
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let png_rows = if has_agent_session && images.is_empty() {
                    debug_rows_containing(grid, 0..grid_size.0, "png")
                } else {
                    Vec::new()
                };
                write_debug_diagnostics(&format!(
                    "session={has_agent_session} cwd={working_directory:?} grid={grid_size:?} \
                     mentions={mention_texts:?} resolved={} placed={:?} first_image_rows={first_image_rows:?} \
                     png_rows_when_unresolved={png_rows:?}",
                    images.len(),
                    placements
                        .iter()
                        .map(|placement| (placement.rows.clone(), placement.columns.clone()))
                        .collect::<Vec<_>>(),
                ));

                placements
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
            },
        ))
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

/// Temporary: appends to `%TEMP%\warp-inline-images-debug.txt` whenever the result changes.
#[cfg(feature = "local_fs")]
fn write_debug_diagnostics(diagnostics: &str) {
    thread_local! {
        static LAST: RefCell<String> = const { RefCell::new(String::new()) };
    }
    LAST.with(|last| {
        if *last.borrow() == diagnostics {
            return;
        }
        diagnostics.clone_into(&mut last.borrow_mut());
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("warp-inline-images-debug.txt"))
        {
            let _ = writeln!(file, "{seconds} {diagnostics}");
        }
    });
}
