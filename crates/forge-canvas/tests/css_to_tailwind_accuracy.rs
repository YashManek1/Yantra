//! # forge-canvas: CSS-to-Tailwind Translation Accuracy Tests
//!
//! Validates `css_props_to_tailwind` against a 30-entry golden table covering
//! exact lookups, spacing-scale lookups, arbitrary-value bracket syntax, and
//! properties that must fall through to `leftover_inline`. Also asserts that
//! overall translation accuracy meets the 70% minimum threshold.
//!
//! ## Input
//! - `CssDeclaration` values constructed from static string slices
//!
//! ## Output
//! - Test assertion results comparing `TailwindTranslation` fields against
//!   expected classes and leftover behaviour
//!
//! ## Related
//! - `forge-canvas::css_to_tailwind` — `css_props_to_tailwind`, `CssDeclaration`

use yantra_canvas::css_to_tailwind::{css_props_to_tailwind, CssDeclaration};

struct GoldenEntry {
    input_property: &'static str,
    input_value: &'static str,
    expected_in_tailwind_classes: Option<&'static str>,
    must_be_leftover: bool,
}

fn golden_table() -> Vec<GoldenEntry> {
    vec![
        GoldenEntry {
            input_property: "display",
            input_value: "flex",
            expected_in_tailwind_classes: Some("flex"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "display",
            input_value: "block",
            expected_in_tailwind_classes: Some("block"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "display",
            input_value: "none",
            expected_in_tailwind_classes: Some("hidden"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "display",
            input_value: "inline-flex",
            expected_in_tailwind_classes: Some("inline-flex"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "display",
            input_value: "grid",
            expected_in_tailwind_classes: Some("grid"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "flex-direction",
            input_value: "column",
            expected_in_tailwind_classes: Some("flex-col"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "flex-direction",
            input_value: "row",
            expected_in_tailwind_classes: Some("flex-row"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "justify-content",
            input_value: "center",
            expected_in_tailwind_classes: Some("justify-center"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "justify-content",
            input_value: "space-between",
            expected_in_tailwind_classes: Some("justify-between"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "align-items",
            input_value: "center",
            expected_in_tailwind_classes: Some("items-center"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "text-align",
            input_value: "center",
            expected_in_tailwind_classes: Some("text-center"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "text-align",
            input_value: "left",
            expected_in_tailwind_classes: Some("text-left"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "font-weight",
            input_value: "bold",
            expected_in_tailwind_classes: Some("font-bold"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "font-weight",
            input_value: "600",
            expected_in_tailwind_classes: Some("font-semibold"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "font-size",
            input_value: "16px",
            expected_in_tailwind_classes: Some("text-base"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "font-size",
            input_value: "14px",
            expected_in_tailwind_classes: Some("text-sm"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "font-size",
            input_value: "24px",
            expected_in_tailwind_classes: Some("text-2xl"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "border-radius",
            input_value: "4px",
            expected_in_tailwind_classes: Some("rounded"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "border-radius",
            input_value: "8px",
            expected_in_tailwind_classes: Some("rounded-lg"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "position",
            input_value: "relative",
            expected_in_tailwind_classes: Some("relative"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "position",
            input_value: "absolute",
            expected_in_tailwind_classes: Some("absolute"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "color",
            input_value: "#ffffff",
            expected_in_tailwind_classes: Some("text-white"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "background-color",
            input_value: "#000000",
            expected_in_tailwind_classes: Some("bg-black"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "padding",
            input_value: "4px",
            expected_in_tailwind_classes: Some("p-1"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "padding",
            input_value: "8px",
            expected_in_tailwind_classes: Some("p-2"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "padding",
            input_value: "16px",
            expected_in_tailwind_classes: Some("p-4"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "padding",
            input_value: "24px",
            expected_in_tailwind_classes: Some("p-6"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "margin",
            input_value: "0",
            expected_in_tailwind_classes: Some("m-0"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "margin",
            input_value: "16px",
            expected_in_tailwind_classes: Some("m-4"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "width",
            input_value: "240px",
            expected_in_tailwind_classes: Some("w-[240px]"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "color",
            input_value: "#123456",
            expected_in_tailwind_classes: Some("text-[#123456]"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "background-color",
            input_value: "#1a1a1a",
            expected_in_tailwind_classes: Some("bg-[#1a1a1a]"),
            must_be_leftover: false,
        },
        GoldenEntry {
            input_property: "transform",
            input_value: "rotate(45deg)",
            expected_in_tailwind_classes: None,
            must_be_leftover: true,
        },
        GoldenEntry {
            input_property: "animation",
            input_value: "spin 1s linear",
            expected_in_tailwind_classes: None,
            must_be_leftover: true,
        },
    ]
}

#[test]
fn test_all_golden_entries_produce_non_empty_output() {
    let table = golden_table();
    for golden_entry in &table {
        let css_declarations = vec![CssDeclaration::new(
            golden_entry.input_property,
            golden_entry.input_value,
        )];
        let translation = css_props_to_tailwind(&css_declarations);
        let has_tailwind_output = !translation.tailwind_classes.is_empty();
        let has_leftover_output = !translation.leftover_inline.is_empty();
        assert!(
            has_tailwind_output || has_leftover_output,
            "entry '{}:{}' produced neither tailwind_classes nor leftover_inline",
            golden_entry.input_property,
            golden_entry.input_value
        );
    }
}

#[test]
fn test_translation_accuracy_meets_minimum() {
    let table = golden_table();
    let non_leftover_entries: Vec<&GoldenEntry> = table
        .iter()
        .filter(|golden_entry| !golden_entry.must_be_leftover)
        .collect();

    let total_non_leftover = non_leftover_entries.len();
    let entries_with_tailwind_class: usize = non_leftover_entries
        .iter()
        .filter(|golden_entry| {
            let css_declarations = vec![CssDeclaration::new(
                golden_entry.input_property,
                golden_entry.input_value,
            )];
            let translation = css_props_to_tailwind(&css_declarations);
            !translation.tailwind_classes.is_empty()
        })
        .count();

    let accuracy_percentage = (entries_with_tailwind_class * 100) / total_non_leftover;
    assert!(
        accuracy_percentage >= 70,
        "translation accuracy {accuracy_percentage}% is below the 70% minimum ({entries_with_tailwind_class}/{total_non_leftover} entries produced a Tailwind class)"
    );
}

#[test]
fn test_exact_class_matches_in_golden_table() {
    let table = golden_table();
    for golden_entry in &table {
        let Some(expected_tailwind_class) = golden_entry.expected_in_tailwind_classes else {
            continue;
        };
        if golden_entry.must_be_leftover {
            continue;
        }
        let css_declarations = vec![CssDeclaration::new(
            golden_entry.input_property,
            golden_entry.input_value,
        )];
        let translation = css_props_to_tailwind(&css_declarations);
        assert!(
            translation
                .tailwind_classes
                .iter()
                .any(|class| class == expected_tailwind_class),
            "expected Tailwind class '{}' for '{}:{}' but got {:?}",
            expected_tailwind_class,
            golden_entry.input_property,
            golden_entry.input_value,
            translation.tailwind_classes
        );
    }
}

#[test]
fn test_leftover_entries_go_to_leftover_inline() {
    let table = golden_table();
    for golden_entry in table.iter().filter(|entry| entry.must_be_leftover) {
        let css_declarations = vec![CssDeclaration::new(
            golden_entry.input_property,
            golden_entry.input_value,
        )];
        let translation = css_props_to_tailwind(&css_declarations);
        assert!(
            !translation.leftover_inline.is_empty(),
            "entry '{}:{}' must appear in leftover_inline but tailwind_classes={:?} leftover={:?}",
            golden_entry.input_property,
            golden_entry.input_value,
            translation.tailwind_classes,
            translation.leftover_inline
        );
        assert!(
            translation.tailwind_classes.is_empty(),
            "entry '{}:{}' must NOT appear in tailwind_classes when must_be_leftover=true",
            golden_entry.input_property,
            golden_entry.input_value
        );
    }
}

#[test]
fn test_empty_declarations_return_empty_output() {
    let translation = css_props_to_tailwind(&[]);
    assert!(
        translation.tailwind_classes.is_empty(),
        "empty input must produce empty tailwind_classes"
    );
    assert!(
        translation.leftover_inline.is_empty(),
        "empty input must produce empty leftover_inline"
    );
}

#[test]
fn test_multiple_declarations_all_processed() {
    let css_declarations = vec![
        CssDeclaration::new("display", "flex"),
        CssDeclaration::new("padding", "8px"),
        CssDeclaration::new("transform", "rotate(45deg)"),
        CssDeclaration::new("animation", "spin 1s linear"),
        CssDeclaration::new("font-weight", "bold"),
    ];
    let translation = css_props_to_tailwind(&css_declarations);
    let total_output_count = translation.tailwind_classes.len() + translation.leftover_inline.len();
    assert_eq!(
        total_output_count, 5,
        "five input declarations must produce exactly five combined output entries, got tailwind={:?} leftover={:?}",
        translation.tailwind_classes,
        translation.leftover_inline
    );
}
