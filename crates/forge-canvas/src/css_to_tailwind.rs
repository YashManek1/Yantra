//! # forge-canvas::css_to_tailwind: CSS Declaration → Tailwind Class Mapper
//!
//! Translates a list of CSS declarations into Tailwind utility classes,
//! falling back to arbitrary-value syntax (`text-[#abcdef]`) for properties
//! we know how to express arbitrarily, and to inline `style={{}}` for
//! properties without any Tailwind equivalent.
//!
//! ## Input
//! - `Vec<CssDeclaration>` — parsed CSS declarations (property + value)
//!
//! ## Output
//! - `TailwindTranslation` — Tailwind classes plus leftover declarations
//!   that should remain as inline `style` attributes
//!
//! ## Related
//! - `forge-canvas::tsx_writer` — primary caller; emits `className` + `style`
//! - `forge-canvas::dom` — `DomNode.inline_style` is the upstream source

use serde::{Deserialize, Serialize};

/// A single CSS declaration: `property: value;`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssDeclaration {
    /// Lower-case CSS property name (e.g. `color`, `padding-top`).
    pub property: String,
    /// Raw CSS value as written (e.g. `#ff0000`, `8px`).
    pub value: String,
}

impl CssDeclaration {
    /// Helper to construct from string slices.
    pub fn new(property: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            property: property.into().to_lowercase(),
            value: value.into(),
        }
    }
}

/// Result of translating a set of declarations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailwindTranslation {
    /// Tailwind utility classes to add to the element's `className`.
    pub tailwind_classes: Vec<String>,
    /// Declarations with no Tailwind equivalent; emit as inline `style`.
    pub leftover_inline: Vec<CssDeclaration>,
}

/// Translates CSS declarations into Tailwind classes + leftover inline styles.
///
/// Lookup proceeds per declaration:
/// 1. Try the exact-value match table (covers common cases like `color: white`,
///    `padding: 8px`, `display: flex`).
/// 2. For properties with a known arbitrary-value pattern (color, padding,
///    margin, width, height, font-size, border-radius), emit
///    `<prefix>-[<value>]` (e.g. `text-[#abcdef]`, `p-[9px]`).
/// 3. Otherwise, leave the declaration in `leftover_inline`.
pub fn css_props_to_tailwind(declarations: &[CssDeclaration]) -> TailwindTranslation {
    let mut tailwind_classes = Vec::new();
    let mut leftover_inline = Vec::new();

    for declaration in declarations {
        let normalized_value = declaration.value.trim().to_lowercase();
        if let Some(exact_class) = exact_lookup(&declaration.property, &normalized_value) {
            tailwind_classes.push(exact_class);
            continue;
        }
        if let Some(arbitrary_class) =
            arbitrary_value(&declaration.property, declaration.value.trim())
        {
            tailwind_classes.push(arbitrary_class);
            continue;
        }
        leftover_inline.push(declaration.clone());
    }

    TailwindTranslation {
        tailwind_classes,
        leftover_inline,
    }
}

fn exact_lookup(property: &str, normalized_value: &str) -> Option<String> {
    if let Some(spacing) = spacing_lookup(property, normalized_value) {
        return Some(spacing);
    }
    let class_name = match (property, normalized_value) {
        ("color", "white" | "#ffffff" | "#fff") => "text-white",
        ("color", "black" | "#000000" | "#000") => "text-black",
        ("background-color", "white" | "#ffffff") => "bg-white",
        ("background-color", "black" | "#000000") => "bg-black",
        ("background-color", "transparent") => "bg-transparent",

        ("display", "flex") => "flex",
        ("display", "inline-flex") => "inline-flex",
        ("display", "grid") => "grid",
        ("display", "block") => "block",
        ("display", "inline") => "inline",
        ("display", "inline-block") => "inline-block",
        ("display", "none") => "hidden",

        ("flex-direction", "row") => "flex-row",
        ("flex-direction", "column") => "flex-col",
        ("justify-content", "center") => "justify-center",
        ("justify-content", "flex-start") => "justify-start",
        ("justify-content", "flex-end") => "justify-end",
        ("justify-content", "space-between") => "justify-between",
        ("justify-content", "space-around") => "justify-around",
        ("align-items", "center") => "items-center",
        ("align-items", "flex-start") => "items-start",
        ("align-items", "flex-end") => "items-end",
        ("align-items", "stretch") => "items-stretch",

        ("text-align", "left") => "text-left",
        ("text-align", "center") => "text-center",
        ("text-align", "right") => "text-right",
        ("text-align", "justify") => "text-justify",

        ("font-size", "12px") => "text-xs",
        ("font-size", "14px") => "text-sm",
        ("font-size", "16px") => "text-base",
        ("font-size", "18px") => "text-lg",
        ("font-size", "20px") => "text-xl",
        ("font-size", "24px") => "text-2xl",
        ("font-size", "30px") => "text-3xl",
        ("font-size", "36px") => "text-4xl",

        ("font-weight", "300") => "font-light",
        ("font-weight", "400" | "normal") => "font-normal",
        ("font-weight", "500") => "font-medium",
        ("font-weight", "600") => "font-semibold",
        ("font-weight", "700" | "bold") => "font-bold",

        ("border-radius", "0") => "rounded-none",
        ("border-radius", "2px") => "rounded-sm",
        ("border-radius", "4px") => "rounded",
        ("border-radius", "6px") => "rounded-md",
        ("border-radius", "8px") => "rounded-lg",
        ("border-radius", "12px") => "rounded-xl",
        ("border-radius", "9999px") => "rounded-full",

        ("position", "static") => "static",
        ("position", "relative") => "relative",
        ("position", "absolute") => "absolute",
        ("position", "fixed") => "fixed",
        ("position", "sticky") => "sticky",

        _ => return None,
    };
    Some(class_name.to_owned())
}

fn spacing_lookup(property: &str, normalized_value: &str) -> Option<String> {
    let scale_step = match normalized_value {
        "0" => "0",
        "4px" => "1",
        "8px" => "2",
        "12px" => "3",
        "16px" => "4",
        "20px" => "5",
        "24px" => "6",
        "32px" => "8",
        "48px" => "12",
        "64px" => "16",
        _ => return None,
    };
    let prefix = match property {
        "padding" => "p",
        "padding-top" => "pt",
        "padding-right" => "pr",
        "padding-bottom" => "pb",
        "padding-left" => "pl",
        "margin" => "m",
        "margin-top" => "mt",
        "margin-right" => "mr",
        "margin-bottom" => "mb",
        "margin-left" => "ml",
        _ => return None,
    };
    Some(format!("{prefix}-{scale_step}"))
}

fn arbitrary_value(property: &str, raw_value: &str) -> Option<String> {
    if raw_value.is_empty() {
        return None;
    }
    let bracketed = format!("[{raw_value}]");
    match property {
        "color" => Some(format!("text-{bracketed}")),
        "background-color" => Some(format!("bg-{bracketed}")),
        "font-size" => Some(format!("text-{bracketed}")),
        "padding" => Some(format!("p-{bracketed}")),
        "padding-top" => Some(format!("pt-{bracketed}")),
        "padding-right" => Some(format!("pr-{bracketed}")),
        "padding-bottom" => Some(format!("pb-{bracketed}")),
        "padding-left" => Some(format!("pl-{bracketed}")),
        "margin" => Some(format!("m-{bracketed}")),
        "margin-top" => Some(format!("mt-{bracketed}")),
        "margin-right" => Some(format!("mr-{bracketed}")),
        "margin-bottom" => Some(format!("mb-{bracketed}")),
        "margin-left" => Some(format!("ml-{bracketed}")),
        "width" => Some(format!("w-{bracketed}")),
        "height" => Some(format!("h-{bracketed}")),
        "border-radius" => Some(format!("rounded-{bracketed}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_color_white() {
        let decls = vec![CssDeclaration::new("color", "white")];
        let translation = css_props_to_tailwind(&decls);
        assert_eq!(translation.tailwind_classes, vec!["text-white"]);
        assert!(translation.leftover_inline.is_empty());
    }

    #[test]
    fn exact_match_padding_8px() {
        let decls = vec![CssDeclaration::new("padding", "8px")];
        let translation = css_props_to_tailwind(&decls);
        assert_eq!(translation.tailwind_classes, vec!["p-2"]);
    }

    #[test]
    fn exact_match_display_flex() {
        let decls = vec![CssDeclaration::new("display", "flex")];
        let translation = css_props_to_tailwind(&decls);
        assert_eq!(translation.tailwind_classes, vec!["flex"]);
    }

    #[test]
    fn arbitrary_value_color_hex() {
        let decls = vec![CssDeclaration::new("color", "#abcdef")];
        let translation = css_props_to_tailwind(&decls);
        assert_eq!(translation.tailwind_classes, vec!["text-[#abcdef]"]);
    }

    #[test]
    fn arbitrary_value_width_pixels() {
        let decls = vec![CssDeclaration::new("width", "240px")];
        let translation = css_props_to_tailwind(&decls);
        assert_eq!(translation.tailwind_classes, vec!["w-[240px]"]);
    }

    #[test]
    fn unmapped_property_falls_through_to_inline() {
        let decls = vec![CssDeclaration::new("filter", "blur(5px)")];
        let translation = css_props_to_tailwind(&decls);
        assert!(translation.tailwind_classes.is_empty());
        assert_eq!(translation.leftover_inline.len(), 1);
        assert_eq!(translation.leftover_inline[0].property, "filter");
    }

    #[test]
    fn multiple_declarations_translate_independently() {
        let decls = vec![
            CssDeclaration::new("color", "black"),
            CssDeclaration::new("padding", "8px"),
            CssDeclaration::new("filter", "blur(5px)"),
        ];
        let translation = css_props_to_tailwind(&decls);
        assert_eq!(translation.tailwind_classes.len(), 2);
        assert_eq!(translation.leftover_inline.len(), 1);
    }
}
