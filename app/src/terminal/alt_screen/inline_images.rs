//! Inline image thumbnails for full-screen CLI agents such as Claude Code.
//!
//! Agents print image paths as plain text and repaint their whole screen constantly, so images
//! sent through terminal image protocols would be painted over. Instead, Warp finds image paths in
//! the visible grid and draws thumbnails itself, in blank cells next to the text that mentions
//! them.

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use warpui::assets::asset_cache::{AssetSource, LocalFileContentVersion};
use warpui::geometry::vector::Vector2F;

use crate::terminal::model::cell::Flags;
use crate::terminal::model::grid::grid_handler::GridHandler;
use crate::terminal::model::index::Point;

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use std::cell::RefCell;
        use std::collections::HashMap;
        use std::time::{Duration, Instant};

        use url::Url;
        use warp_util::path::CleanPathResult;

        use crate::terminal::ShellLaunchData;
        use crate::util::file::{ShellPathType, absolute_path_if_valid};
        use crate::util::openable_file_type::is_supported_image_file;
    }
}

/// Thumbnail heights to try, in terminal rows, from most to least preferred.
const THUMBNAIL_ROW_HEIGHTS: [usize; 2] = [4, 2];
/// Width / height of a thumbnail box, in pixels.
const THUMBNAIL_ASPECT_RATIO: f32 = 1.6;
/// Blank columns kept between a thumbnail and the nearest text.
const GAP_COLUMNS: usize = 1;
const MAX_THUMBNAILS: usize = 8;
#[cfg(feature = "local_fs")]
const MAX_CACHED_PATHS: usize = 512;
/// How long a path lookup is trusted before the filesystem is checked again. Agents often mention
/// a file just before writing it, and may overwrite an image in place.
#[cfg(feature = "local_fs")]
const PATH_RECHECK_INTERVAL: Duration = Duration::from_secs(2);

static IMAGE_PATH_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"[^\s"'`<>()\[\]{}|,;]+\.(?i:png|jpe?g|gif|webp|svg)\b"#)
        .expect("image path regex should compile")
});

/// Text that looks like an image path, found in one grid row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePathMention {
    pub row: usize,
    /// Grid columns covered by the text.
    pub columns: Range<usize>,
    pub text: String,
}

/// A mention whose text resolved to an image file on disk.
#[derive(Clone, Debug)]
pub struct ResolvedImage {
    pub mention: ImagePathMention,
    pub path: PathBuf,
    pub content_version: Option<LocalFileContentVersion>,
}

impl ResolvedImage {
    pub fn asset_source(&self) -> AssetSource {
        AssetSource::LocalFile {
            path: self.path.to_string_lossy().into_owned(),
            content_version: self.content_version,
        }
    }
}

/// Where a thumbnail goes, in grid cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailPlacement {
    /// Index into the resolved images passed to [`place_thumbnails`].
    pub image_index: usize,
    pub rows: Range<usize>,
    pub columns: Range<usize>,
}

/// A thumbnail for the alt screen to draw.
pub struct InlineThumbnail {
    pub placement: ThumbnailPlacement,
    pub path: PathBuf,
    pub source: AssetSource,
}

/// Finds the thumbnails to draw for the grid's current contents, given the cell size.
///
/// The alt screen calls this while painting rather than while rendering, because a full-screen app
/// can change the grid many times between renders.
pub type InlineThumbnailFinder = Box<dyn Fn(&GridHandler, Vector2F) -> Vec<InlineThumbnail>>;

/// Finds image paths in `rows` of `grid`: text that looks like an image path, and OSC 8 links to
/// local image files (Claude Code labels pasted images as `[Image #1]`, linked to a saved copy).
pub fn find_image_path_mentions(grid: &GridHandler, rows: Range<usize>) -> Vec<ImagePathMention> {
    let mut mentions = Vec::new();
    for row in rows {
        let Some(row_text) = RowText::from_grid(grid, row) else {
            continue;
        };
        mentions.extend(row_text.image_path_mentions(row));
        #[cfg(feature = "local_fs")]
        mentions.extend(row_text.image_link_mentions(row));
    }
    mentions
}

/// Text of one grid row, plus the grid column of every byte of that text.
struct RowText {
    text: String,
    byte_columns: Vec<usize>,
    /// OSC 8 hyperlinks in the row, as (grid columns, URI).
    links: Vec<(Range<usize>, String)>,
}

impl RowText {
    fn from_grid(grid: &GridHandler, row: usize) -> Option<Self> {
        let cells = grid.row(row)?;
        let mut text = String::with_capacity(cells.len());
        let mut byte_columns = Vec::with_capacity(cells.len());
        let mut links: Vec<(Range<usize>, String)> = Vec::new();
        let mut current_link = None;
        for column in 0..cells.len() {
            let cell = &cells[column];

            let link_id = cell.hyperlink_id();
            if link_id.is_some() && link_id == current_link {
                if let Some((columns, _)) = links.last_mut() {
                    columns.end = column + 1;
                }
            } else if link_id.is_some()
                && let Some(uri) = grid.hyperlink_uri_at_point(Point::new(row, column))
            {
                links.push((column..column + 1, uri.to_owned()));
            }
            current_link = link_id;

            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            text.push(cell.c);
            byte_columns.extend(std::iter::repeat_n(column, cell.c.len_utf8()));
        }
        Some(Self {
            text,
            byte_columns,
            links,
        })
    }

    #[cfg(test)]
    fn from_text(text: &str) -> Self {
        let mut byte_columns = Vec::with_capacity(text.len());
        for (column, c) in text.chars().enumerate() {
            byte_columns.extend(std::iter::repeat_n(column, c.len_utf8()));
        }
        Self {
            text: text.to_owned(),
            byte_columns,
            links: Vec::new(),
        }
    }

    #[cfg(feature = "local_fs")]
    fn image_link_mentions(&self, row: usize) -> impl Iterator<Item = ImagePathMention> + '_ {
        self.links.iter().filter_map(move |(columns, uri)| {
            let url = Url::parse(uri).ok().filter(|url| url.scheme() == "file")?;
            let path = url.to_file_path().ok()?;
            Some(ImagePathMention {
                row,
                columns: columns.clone(),
                text: path.to_string_lossy().into_owned(),
            })
        })
    }

    fn image_path_mentions(&self, row: usize) -> impl Iterator<Item = ImagePathMention> + '_ {
        IMAGE_PATH_TOKEN.find_iter(&self.text).filter_map(move |token| {
            // URLs are not local files; skip them rather than probing the filesystem.
            if token.as_str().contains("://") {
                return None;
            }
            let start = *self.byte_columns.get(token.start())?;
            let end = *self.byte_columns.get(token.end() - 1)? + 1;
            Some(ImagePathMention {
                row,
                columns: start..end,
                text: token.as_str().to_owned(),
            })
        })
    }
}

/// Chooses a spot for each image's thumbnail.
///
/// Thumbnails sit at the right edge of the grid, starting on the mention's row, but only over
/// blank cells so they never cover text. If there's no room for any size, the image gets no
/// thumbnail. Each image file gets at most one thumbnail.
pub fn place_thumbnails(
    images: &[ResolvedImage],
    grid_size: (usize, usize),
    cell_size: Vector2F,
    is_blank_cells: impl Fn(usize, Range<usize>) -> bool,
) -> Vec<ThumbnailPlacement> {
    let (grid_rows, grid_columns) = grid_size;
    let mut placements: Vec<ThumbnailPlacement> = Vec::new();
    let mut placed_paths = HashSet::new();

    for (image_index, image) in images.iter().enumerate() {
        if placements.len() >= MAX_THUMBNAILS {
            break;
        }
        if placed_paths.contains(&image.path) {
            continue;
        }

        let placement = THUMBNAIL_ROW_HEIGHTS.iter().find_map(|&height| {
            let width = thumbnail_width_in_columns(height, cell_size);
            let first_column = grid_columns.checked_sub(width)?;
            let blank_columns = first_column.checked_sub(GAP_COLUMNS)?..grid_columns;
            let rows = image.mention.row..image.mention.row + height;
            let fits = rows.end <= grid_rows
                && !placements
                    .iter()
                    .any(|placed| ranges_overlap(&placed.rows, &rows))
                && rows
                    .clone()
                    .all(|row| is_blank_cells(row, blank_columns.clone()));
            fits.then(|| ThumbnailPlacement {
                image_index,
                rows,
                columns: first_column..grid_columns,
            })
        });

        if let Some(placement) = placement {
            placed_paths.insert(image.path.clone());
            placements.push(placement);
        }
    }
    placements
}

fn thumbnail_width_in_columns(height_in_rows: usize, cell_size: Vector2F) -> usize {
    if cell_size.x() <= 0. {
        return 0;
    }
    let width_px = height_in_rows as f32 * cell_size.y() * THUMBNAIL_ASPECT_RATIO;
    (width_px / cell_size.x()).ceil() as usize
}

fn ranges_overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// Returns whether every cell of `columns` in `row` is empty.
pub fn is_blank(grid: &GridHandler, row: usize, columns: Range<usize>) -> bool {
    let Some(cells) = grid.row(row) else {
        return false;
    };
    columns.end <= cells.len()
        && cells[columns]
            .iter()
            .all(|cell| cell.c == ' ' || cell.c == '\0')
}

#[cfg(feature = "local_fs")]
#[derive(Clone)]
struct CachedLookup {
    image: Option<(PathBuf, Option<LocalFileContentVersion>)>,
    checked_at: Instant,
}

/// Caches which mention texts point at image files, so the filesystem isn't hit on every frame.
#[cfg(feature = "local_fs")]
#[derive(Default)]
pub struct ImagePathCache {
    lookups: RefCell<HashMap<(String, String), CachedLookup>>,
}

#[cfg(feature = "local_fs")]
impl ImagePathCache {
    pub fn resolve(
        &self,
        mention: ImagePathMention,
        working_directory: &str,
        shell_launch_data: Option<&ShellLaunchData>,
    ) -> Option<ResolvedImage> {
        let key = (working_directory.to_owned(), mention.text.clone());
        let cached = self
            .lookups
            .borrow()
            .get(&key)
            .filter(|lookup| lookup.checked_at.elapsed() < PATH_RECHECK_INTERVAL)
            .cloned();

        let lookup = cached.unwrap_or_else(|| {
            let image = resolve_image_path(&mention.text, working_directory, shell_launch_data)
                .map(|path| {
                    let content_version = LocalFileContentVersion::for_path(&path);
                    (path, content_version)
                });
            let lookup = CachedLookup {
                image,
                checked_at: Instant::now(),
            };
            let mut lookups = self.lookups.borrow_mut();
            if lookups.len() >= MAX_CACHED_PATHS {
                lookups.clear();
            }
            lookups.insert(key, lookup.clone());
            lookup
        });

        let (path, content_version) = lookup.image?;
        Some(ResolvedImage {
            mention,
            path,
            content_version,
        })
    }
}

#[cfg(feature = "local_fs")]
fn resolve_image_path(
    text: &str,
    working_directory: &str,
    shell_launch_data: Option<&ShellLaunchData>,
) -> Option<PathBuf> {
    let clean_path = CleanPathResult {
        path: text.to_owned(),
        line_and_column_num: None,
    };
    absolute_path_if_valid(
        &clean_path,
        ShellPathType::ShellNative(working_directory.to_owned()),
        shell_launch_data,
    )
    .filter(|path| path.is_file() && is_supported_image_file(path))
}

#[cfg(test)]
#[path = "inline_images_tests.rs"]
mod tests;
