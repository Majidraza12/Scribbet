//! M9 fuzz-style property tests: parsers over user-controlled files must be
//! total. Profiles are user-edited TOML (docs/06 T6) and settings survive
//! hand edits; both must reject garbage with an error, never a panic.

use od_storage::{ProfileToml, Settings};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Arbitrary text through the strict profile parser: Ok or Err, no panic.
    #[test]
    fn profile_toml_parser_is_total(input in any::<String>()) {
        let _ = toml::from_str::<ProfileToml>(&input);
    }

    /// Arbitrary text through the settings parser: Ok or Err, no panic.
    #[test]
    fn settings_json_parser_is_total(input in any::<String>()) {
        let _ = serde_json::from_str::<Settings>(&input);
    }

    /// TOML that *parses* must also resolve without panicking (against an
    /// empty in-memory dictionary), whatever field values it carries.
    #[test]
    fn parsed_profiles_resolve_without_panic(input in any::<String>()) {
        if let Ok(p) = toml::from_str::<ProfileToml>(&input) {
            let repo = od_storage::SqliteDictionaryRepo::open_in_memory().unwrap();
            let _ = od_storage::resolve_profile(&p, &repo);
        }
    }
}
