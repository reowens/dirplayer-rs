//! Native integration test: dump the 199 catalogue-icon bitmaps from
//! `cc_furniture_small.cct` as raw PNGs plus deterministic metadata sidecars.
//!
//! Run with an output root outside Furni's runtime asset tree:
//!   CASTS_ROOT=/abs/path/upstream/cokemusic-casts/client2 \
//!   OUTPUT_ROOT=/tmp/dirplayer-furniture-small \
//!   cargo test -p vm-rust --test dump_furniture_small_bitmaps -- --nocapture
//!
//! Output:
//!   - <OUTPUT_ROOT>/furniture-small/data/<member_name>.png
//!   - <OUTPUT_ROOT>/furniture-small/_cc_furniture_small_members.json
//!   - <OUTPUT_ROOT>/furniture-small/_palettes.json

#![cfg(not(target_arch = "wasm32"))]

mod support;

use support::furniture_bitmap_dumper::{FurnitureBitmapProfile, dump_furniture_profile};

#[test]
fn dump_furniture_small_cct_bitmaps() {
    async_std::task::block_on(async {
        let report = dump_furniture_profile(FurnitureBitmapProfile {
            source_cct: "cc_furniture_small.cct",
            output_subdirectory: "furniture-small",
            members_sidecar: "_cc_furniture_small_members.json",
            dumper_name: "dump_furniture_small_bitmaps",
        })
        .await;

        assert_eq!(report.named_bitmap_members, 199);
        assert_eq!(report.palette_members, 86);
        assert_eq!(report.duplicate_names, 0);
        assert_eq!(report.empty_named_bitmaps, 0);
        assert!(
            report.folded_name_collisions.is_empty(),
            "case-folded member-name collisions: {:?}",
            report.folded_name_collisions
        );
        assert!(
            report.decode_failures.is_empty(),
            "furniture-small decode failures: {}",
            report.decode_failures.join(", ")
        );
        assert_eq!(report.member_entries, 199);
        assert_eq!(report.pngs_written, 199);
        assert_eq!(report.emitted_names.len(), 199);
        assert!(report.emitted_names.is_sorted());
    });
}
