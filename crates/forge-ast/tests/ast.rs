use std::path::Path;

use proptest::prelude::*;
use rusqlite::Connection;
use yantra_ast::{
    extract_calls, extract_imports, extract_symbols, insert_symbol, parse_file, query_symbol,
    LanguageKind, Symbol, SymbolKind,
};
use yantra_core::SymbolId;

#[test]
fn rust_fixture_extracts_eight_symbols_with_correct_kinds() {
    let parsed = parse_file(Path::new("tests/fixtures/rust/auth_service.rs")).unwrap();
    let symbols = extract_symbols(&parsed).unwrap();
    let symbol_kinds = symbols
        .iter()
        .map(|symbol| (symbol.name.as_str(), symbol.kind))
        .collect::<Vec<_>>();

    assert_eq!(symbols.len(), 8);
    assert!(symbol_kinds.contains(&("nested", SymbolKind::Module)));
    assert!(symbol_kinds.contains(&("TOKEN_TTL_SECONDS", SymbolKind::Const)));
    assert!(symbol_kinds.contains(&("AUTH_HEADER", SymbolKind::Static)));
    assert!(symbol_kinds.contains(&("AuthService", SymbolKind::Struct)));
    assert!(symbol_kinds.contains(&("AuthError", SymbolKind::Enum)));
    assert!(symbol_kinds.contains(&("TokenVerifier", SymbolKind::Trait)));
    assert!(symbol_kinds.contains(&("AuthService", SymbolKind::Impl)));
    assert!(symbol_kinds.contains(&("validate_token", SymbolKind::Function)));
}

#[test]
fn python_fixture_extracts_symbols_calls_and_imports() {
    let parsed = parse_file(Path::new("tests/fixtures/python/auth_service.py")).unwrap();
    let symbols = extract_symbols(&parsed).unwrap();
    let calls = extract_calls(&parsed).unwrap();
    let imports = extract_imports(&parsed).unwrap();

    assert_eq!(parsed.language, LanguageKind::Python);
    assert!(symbols
        .iter()
        .any(|symbol| symbol.name == "AuthService" && symbol.kind == SymbolKind::Class));
    assert!(symbols
        .iter()
        .any(|symbol| symbol.name == "authenticate" && symbol.kind == SymbolKind::Method));
    assert!(symbols
        .iter()
        .any(|symbol| symbol.name == "validate_token" && symbol.kind == SymbolKind::Function));
    assert!(calls
        .iter()
        .any(|call_site| call_site.callee_name == "validate_token"));
    assert!(imports.iter().any(|import| import.module_path == "typing"));
}

#[test]
fn typescript_fixture_extracts_symbols_calls_and_imports() {
    let parsed = parse_file(Path::new("tests/fixtures/typescript/auth_service.ts")).unwrap();
    let symbols = extract_symbols(&parsed).unwrap();
    let calls = extract_calls(&parsed).unwrap();
    let imports = extract_imports(&parsed).unwrap();

    assert_eq!(parsed.language, LanguageKind::TypeScript);
    assert!(symbols
        .iter()
        .any(|symbol| symbol.name == "AuthService" && symbol.kind == SymbolKind::Class));
    assert!(symbols
        .iter()
        .any(|symbol| symbol.name == "authenticate" && symbol.kind == SymbolKind::Method));
    assert!(symbols
        .iter()
        .any(|symbol| symbol.name == "validateToken" && symbol.kind == SymbolKind::Function));
    assert!(calls
        .iter()
        .any(|call_site| call_site.callee_name == "validateToken"));
    assert!(imports.iter().any(|import| import.module_path == "fs"));
}

proptest! {
    #[test]
    fn symbol_round_trips_through_database(
        name in "[A-Za-z_][A-Za-z0-9_]{0,20}",
        signature in "[A-Za-z0-9_ ():\\-]{0,40}",
        docstring in "[A-Za-z0-9_ .,]{0,80}",
    ) {
        let database = Connection::open_in_memory().unwrap();
        let symbol = Symbol {
            id: SymbolId::from_parts("src/lib.rs", &name, SymbolKind::Function.as_str()),
            name,
            kind: SymbolKind::Function,
            file_path: "src/lib.rs".into(),
            start_line: 1,
            end_line: 3,
            signature: (!signature.is_empty()).then_some(signature),
            docstring: (!docstring.is_empty()).then_some(docstring),
            language: LanguageKind::Rust,
        };

        insert_symbol(&database, &symbol).unwrap();
        let queried_symbol = query_symbol(&database, &symbol.id).unwrap().unwrap();

        prop_assert_eq!(queried_symbol, symbol);
    }
}
