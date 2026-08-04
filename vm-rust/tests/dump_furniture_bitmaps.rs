//! Native integration test: dump every named bitmap member of
//! `cc_furniture[1].cct` as PNG plus a regPoint metadata sidecar.
//!
//! Run:
//!   CASTS_ROOT=/abs/path/upstream/cokemusic-casts/client2 \
//!   OUTPUT_ROOT=/abs/path/packages/client/public/assets \
//!   cargo test -p vm-rust --test dump_furniture_bitmaps -- --nocapture
//!
//! Output:
//!   - <OUTPUT_ROOT>/furniture/data/<member_name>.png
//!   - <OUTPUT_ROOT>/furniture/_cc_furniture_members.json
//!   - <OUTPUT_ROOT>/furniture/_palettes.json

#![cfg(not(target_arch = "wasm32"))]

mod support;

use support::furniture_bitmap_dumper::{FurnitureBitmapProfile, dump_furniture_profile};

#[test]
fn dump_furniture_cct_bitmaps() {
    async_std::task::block_on(async {
        let report = dump_furniture_profile(FurnitureBitmapProfile {
            source_cct: "cc_furniture[1].cct",
            output_subdirectory: "furniture",
            members_sidecar: "_cc_furniture_members.json",
            dumper_name: "dump_furniture_bitmaps",
        })
        .await;

        assert!(
            report.decode_failures.is_empty(),
            "furniture decode failures: {}",
            report.decode_failures.join(", ")
        );
    });
}
