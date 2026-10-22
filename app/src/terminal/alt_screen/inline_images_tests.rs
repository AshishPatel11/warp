use std::ops::Range;
use std::path::PathBuf;

use warpui::geometry::vector::vec2f;

use super::*;

fn mention_texts(line: &str) -> Vec<(String, Range<usize>)> {
    RowText::from_text(line)
        .image_path_mentions(0)
        .map(|mention| (mention.text, mention.columns))
        .collect()
}

fn image_at_row(row: usize, path: &str) -> ResolvedImage {
    ResolvedImage {
        mention: ImagePathMention {
            row,
            columns: 0..path.len(),
            text: path.to_owned(),
        },
        path: PathBuf::from(path),
        content_version: None,
    }
}

/// 10px wide, 20px tall cells: a 4-row thumbnail is 80px tall and 128px wide, so 13 columns.
const CELL_SIZE: (f32, f32) = (10., 20.);

#[test]
fn finds_windows_unix_and_relative_paths() {
    assert_eq!(
        mention_texts(r"● Saved the chart to C:\Users\me\out\chart.png and docs/logo.SVG."),
        vec![
            (r"C:\Users\me\out\chart.png".to_owned(), 21..46),
            ("docs/logo.SVG".to_owned(), 51..64),
        ]
    );
    assert_eq!(
        mention_texts("see /c/Users/me/shot.jpeg:12"),
        vec![("/c/Users/me/shot.jpeg".to_owned(), 4..25)]
    );
}

#[test]
fn ignores_urls_and_non_image_files() {
    assert!(mention_texts("open https://example.com/cat.png now").is_empty());
    assert!(mention_texts("edited src/main.rs and notes.pngx").is_empty());
}

#[test]
fn maps_columns_past_multibyte_characters() {
    // "●" is 3 bytes but one column, so the path must start at column 2, not byte 4.
    assert_eq!(
        mention_texts("● a.png"),
        vec![("a.png".to_owned(), 2..7)]
    );
}

#[test]
fn finds_file_links_and_ignores_web_links() {
    let image_path = std::env::temp_dir().join("image-cache").join("1.png");
    let image_uri = url::Url::from_file_path(&image_path).unwrap().to_string();
    let row_text = RowText {
        links: vec![
            (2..12, image_uri),
            (20..30, "https://example.com/cat.png".to_owned()),
        ],
        ..RowText::from_text("> [Image #1] what is this?")
    };
    let mentions: Vec<_> = row_text.image_link_mentions(5).collect();
    assert_eq!(
        mentions,
        vec![ImagePathMention {
            row: 5,
            columns: 2..12,
            text: image_path.to_string_lossy().into_owned(),
        }]
    );
}

#[test]
fn places_thumbnail_at_right_edge_of_mention_row() {
    let images = [image_at_row(3, "a.png")];
    let placements = place_thumbnails(
        &images,
        (20, 80),
        vec2f(CELL_SIZE.0, CELL_SIZE.1),
        |_, _| true,
    );
    assert_eq!(
        placements,
        vec![ThumbnailPlacement {
            image_index: 0,
            rows: 3..7,
            columns: 67..80,
        }]
    );
}

#[test]
fn falls_back_to_smaller_thumbnail_when_rows_below_are_busy() {
    let images = [image_at_row(3, "a.png")];
    let placements = place_thumbnails(
        &images,
        (20, 80),
        vec2f(CELL_SIZE.0, CELL_SIZE.1),
        |row, _| row < 5,
    );
    assert_eq!(placements.len(), 1);
    assert_eq!(placements[0].rows, 3..5);
}

#[test]
fn skips_image_when_no_room_or_off_screen() {
    let images = [image_at_row(3, "a.png"), image_at_row(19, "b.png")];
    let placements = place_thumbnails(
        &images,
        (20, 80),
        vec2f(CELL_SIZE.0, CELL_SIZE.1),
        |row, _| row != 3,
    );
    assert!(placements.is_empty());
}

#[test]
fn does_not_overlap_or_repeat_thumbnails() {
    let images = [
        image_at_row(2, "a.png"),
        image_at_row(4, "b.png"),
        image_at_row(8, "a.png"),
    ];
    let placements = place_thumbnails(
        &images,
        (20, 80),
        vec2f(CELL_SIZE.0, CELL_SIZE.1),
        |_, _| true,
    );
    // b.png can't take 4 rows at row 4 (a.png's thumbnail covers rows 2..6), nor 2 rows (4..6).
    // The second mention of a.png is skipped because a.png already has a thumbnail.
    assert_eq!(
        placements
            .iter()
            .map(|placement| (placement.image_index, placement.rows.clone()))
            .collect::<Vec<_>>(),
        vec![(0, 2..6)]
    );
}
