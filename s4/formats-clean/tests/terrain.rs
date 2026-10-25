mod common;
use common::*;
use s4_formats_clean::terrain::*;

fn table(name: &str) -> Vec<i64> {
    let row = rows("terrain.tsv").into_iter().find(|r| r[0] == name).unwrap_or_else(|| panic!("no table {name}"));
    row[1].split(',').map(|v| v.parse().unwrap()).collect()
}

#[test]
fn tables_match_the_table_file() {
    let colors: Vec<i64> = TERRAIN_COLORS.iter().flatten().map(|&v| v as i64).collect();
    assert_eq!(colors, table("colors"), "colors");
    assert_eq!(TEXTURE_CLASS.iter().map(|&v| v as i64).collect::<Vec<_>>(), table("texture_class"));
    assert_eq!(CLASS_PAGE.iter().map(|&v| v as i64).collect::<Vec<_>>(), table("class_page"));
    assert_eq!(TRANSITION_INDEX.iter().map(|&v| v as i64).collect::<Vec<_>>(), table("transition_index"));
    let tr: Vec<i64> = TRANSITIONS.iter().flat_map(|t| [t.0 as i64, t.1 as i64, t.2[0] as i64, t.2[1] as i64, t.2[2] as i64, t.2[3] as i64]).collect();
    assert_eq!(tr, table("transitions"), "transitions");
    assert_eq!(TEXELS_PER_TILE as i64, table("texels_per_tile")[0]);
}

#[test]
fn transition_lookup_for_every_pair() {
    let want = std::fs::read(vectors_dir().join("data/terrain-transition.bin")).unwrap();
    assert_eq!(want.len(), 256 * 256 * 5);
    for a in 0..256usize {
        for b in 0..256usize {
            let at = (a * 256 + b) * 5;
            let expect = if want[at] == 1 { Some([want[at + 1], want[at + 2], want[at + 3], want[at + 4]]) } else { None };
            assert_eq!(transition(a as u8, b as u8), expect, "pair {a}, odd {b}");
        }
    }
}

#[test]
fn minimap_colour_of_tiles() {
    let want = std::fs::read(vectors_dir().join("data/terrain-minimap.bin")).unwrap();
    assert_eq!(want.len(), 256 * 256 * 8 * 3);
    let mut bad = Vec::new();
    for ty in 0..256usize {
        for b2 in 0..256usize {
            for fog in 0..8usize {
                let b3 = ((fog << 3) | ((ty + b2) % 8) | (((ty ^ b2) >> 6) << 6)) as u8;
                let tile = [(ty * 7 + b2) as u8, ty as u8, b2 as u8, b3];
                let at = ((ty * 256 + b2) * 8 + fog) * 3;
                if minimap_color(tile) != [want[at], want[at + 1], want[at + 2]] && bad.len() < 10 {
                    bad.push(format!("tile {tile:?}: got {:?}, expected {:?}", minimap_color(tile), &want[at..at + 3]));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn byte_zero_and_low_fog_bits_do_not_matter() {
    for ty in [0u8, 16, 100, 255] {
        for b2 in [0u8, 8, 0x4a, 0xff] {
            for b3 in 0..=255u8 {
                let a = minimap_color([0, ty, b2, b3]);
                assert_eq!(a, minimap_color([0xaa, ty, b2, b3 & 0x38]), "tile type {ty} b2 {b2} b3 {b3}");
            }
        }
    }
}
