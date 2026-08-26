//! Preview sizing, checked against real samples lifted from a Desktop export.
//!
//! 649 `<img>` tags across the reference's nine pages, reduced to 74 unique
//! (width, height, class) shapes. Each one pairs the dimensions Desktop wrote
//! into `result.json` with the CSS size it wrote into the `style` attribute of
//! the same message's image — so this checks the arithmetic against Desktop's
//! own output rather than against examples we invented.
//!
//! The fixture is committed, so this runs with no reference export on disk.

use serde_json::Value;
use tgx_html::preview::{preview_size, PREVIEW_BOX, STICKER_BOX};

#[test]
fn every_reference_preview_size_is_reproduced() {
    let raw = include_str!("preview_samples.json");
    let samples: Vec<Value> = serde_json::from_str(raw).expect("fixture parses");
    assert!(samples.len() >= 70, "fixture looks truncated");

    let mut checked = 0usize;
    let mut wrong: Vec<String> = Vec::new();

    for s in &samples {
        let w = s["w"].as_i64().unwrap();
        let h = s["h"].as_i64().unwrap();
        let want = (s["css_w"].as_i64().unwrap(), s["css_h"].as_i64().unwrap());
        let cls = s["cls"].as_str().unwrap();

        // Stickers scale into the smaller box; everything else into the
        // standard one.
        let box_size = if cls == "sticker" {
            STICKER_BOX
        } else {
            PREVIEW_BOX
        };
        let (_file, css) = preview_size(w, h, box_size);

        checked += 1;
        if css != want {
            wrong.push(format!("{cls} {w}x{h}: desktop {want:?}, ours {css:?}"));
        }
    }

    assert!(
        wrong.is_empty(),
        "{} of {checked} preview sizes differ:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
    println!("{checked} unique preview sizes reproduced exactly");
}
