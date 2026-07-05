//! TOML profiles: the per-context configuration unit (docs/02 "Profiles").
//!
//! Shipped profiles are embedded in the binary; user profiles are the same
//! TOML format in the config dir and shadow shipped ones by id. Profiles
//! are user-editable text and treated as untrusted input (docs/06 T6):
//! unknown keys are rejected loudly rather than silently ignored, except
//! the documented future `[plugins]` table.

use std::path::PathBuf;
use std::sync::Arc;

use od_core_types::{
    CleanupConfig, DictEntry, FillerConfig, FormatConfig, LanguageHint, PipelineCtx,
    ProfileSnapshot, PunctuationConfig, SymbolsConfig, UserRule, VocabBias,
};
use serde::Deserialize;

use crate::{DictionaryRepo, StorageError};

/// Shipped profile ids, embedded at compile time.
const SHIPPED: &[(&str, &str)] = &[
    ("general", include_str!("../profiles/general.toml")),
    ("email", include_str!("../profiles/email.toml")),
    ("coding", include_str!("../profiles/coding.toml")),
    ("meeting", include_str!("../profiles/meeting.toml")),
    (
        "professional",
        include_str!("../profiles/professional.toml"),
    ),
    ("medical", include_str!("../profiles/medical.toml")),
    ("legal", include_str!("../profiles/legal.toml")),
];

/// Ids of the profiles shipped with the app.
pub fn shipped_profile_ids() -> Vec<&'static str> {
    SHIPPED.iter().map(|(id, _)| *id).collect()
}

/// Loads profiles by id: user TOML files shadow shipped ones.
pub struct ProfileStore {
    /// Directory holding user profile TOMLs (`<dir>/<id>.toml`).
    dir: PathBuf,
}

impl ProfileStore {
    /// A store over the user profile directory (need not exist yet).
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Loads and parses a profile by id.
    pub fn load(&self, id: &str) -> Result<ProfileToml, StorageError> {
        let user_path = self.dir.join(format!("{id}.toml"));
        let text = if user_path.is_file() {
            std::fs::read_to_string(&user_path)?
        } else if let Some((_, embedded)) = SHIPPED.iter().find(|(sid, _)| *sid == id) {
            (*embedded).to_owned()
        } else {
            return Err(StorageError::UnknownProfile(id.to_owned()));
        };
        Ok(toml::from_str(&text)?)
    }

    /// All available profile ids: shipped plus user files, deduplicated.
    pub fn list(&self) -> Vec<String> {
        let mut ids: Vec<String> = shipped_profile_ids()
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        if let Ok(read) = std::fs::read_dir(&self.dir) {
            for entry in read.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "toml") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if !ids.iter().any(|i| i == stem) {
                            ids.push(stem.to_owned());
                        }
                    }
                }
            }
        }
        ids
    }
}

/// Resolves a parsed profile against the dictionary into the immutable
/// per-utterance context: language hint, vocab bias, and the profile
/// snapshot the cleanup chain consumes.
pub fn resolve_profile(
    p: &ProfileToml,
    repo: &dyn DictionaryRepo,
) -> Result<PipelineCtx, StorageError> {
    let language = match p.stt.language.as_str() {
        "auto" => LanguageHint::Auto,
        code => LanguageHint::Fixed(code.to_owned()),
    };

    // "dictionary:NAME" bias sources; anything else is a schema from the
    // future — warn and continue rather than failing the whole profile.
    let mut bias_sets = Vec::new();
    for source in &p.stt.vocab_bias {
        match source.strip_prefix("dictionary:") {
            Some(set) => bias_sets.push(set.to_owned()),
            None => tracing::warn!(source, "unknown vocab bias source; skipped"),
        }
    }
    let vocab = VocabBias {
        terms: repo.vocab_terms(&bias_sets)?,
    };

    let entries: Vec<DictEntry> = repo.entries(&p.dictionaries.sets)?;

    let snapshot = ProfileSnapshot {
        name: p.profile.name.clone(),
        cleanup: p.cleanup.to_config(&p.format),
        dictionary_sets: p.dictionaries.sets.clone(),
        entries,
        rewriter_allowed: p.cloud.rewriter_allowed,
    };

    Ok(PipelineCtx {
        language,
        vocab,
        profile: Arc::new(snapshot),
    })
}

/// Root of the profile TOML schema (docs/02 "Profiles").
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileToml {
    /// `[profile]` identity.
    pub profile: ProfileMeta,
    /// `[stt]` engine hints.
    #[serde(default)]
    pub stt: SttSection,
    /// `[cleanup]` processor chain configuration.
    #[serde(default)]
    pub cleanup: CleanupSection,
    /// `[dictionaries]` set subscriptions.
    #[serde(default)]
    pub dictionaries: DictionariesSection,
    /// `[format]` profile-specific final pass.
    #[serde(default)]
    pub format: FormatSection,
    /// `[cloud]` policy.
    #[serde(default)]
    pub cloud: CloudSection,
    /// `[plugins]` — documented future surface; accepted and ignored.
    #[serde(default)]
    pub plugins: Option<toml::Table>,
}

/// `[profile]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileMeta {
    /// Display name.
    pub name: String,
}

/// `[stt]`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SttSection {
    /// "auto" or an ISO 639-1 code.
    pub language: String,
    /// Bias sources ("dictionary:user", ...).
    pub vocab_bias: Vec<String>,
}

impl Default for SttSection {
    fn default() -> Self {
        Self {
            language: "en".into(),
            vocab_bias: vec!["dictionary:user".into()],
        }
    }
}

/// A bare `enabled` toggle table.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Toggle {
    /// Whether the processor runs.
    pub enabled: bool,
}

impl Default for Toggle {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// `[cleanup]` — every field optional, defaulting to the standard chain.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CleanupSection {
    /// Processor 1 toggle.
    pub whitespace: Toggle,
    /// Processor 2 toggle + extra fillers.
    pub fillers: FillersSection,
    /// Processor 3 toggle.
    pub dictionary: Toggle,
    /// Processor 4 toggle + table.
    pub symbols: SymbolsSection,
    /// Processor 5 configuration.
    pub punctuation: PunctuationSection,
    /// Processor 6 toggle.
    pub segmentation: Toggle,
    /// Processor 7 toggle.
    pub capitalization: Toggle,
    /// Proper nouns for processor 7.
    pub proper_nouns: Vec<String>,
    /// Processor 8 rules, in order.
    pub rules: Vec<RuleSection>,
}

/// `[cleanup.fillers]`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FillersSection {
    /// Whether filler removal runs.
    pub enabled: bool,
    /// Extra fillers (removed unconditionally).
    pub extra: Vec<String>,
}

impl Default for FillersSection {
    fn default() -> Self {
        Self {
            enabled: true,
            extra: Vec::new(),
        }
    }
}

/// `[cleanup.symbols]`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SymbolsSection {
    /// Whether symbol replacement runs.
    pub enabled: bool,
    /// Built-in table name.
    pub table: String,
}

impl Default for SymbolsSection {
    fn default() -> Self {
        Self {
            enabled: true,
            table: "general".into(),
        }
    }
}

/// `[cleanup.punctuation]`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PunctuationSection {
    /// Whether punctuation repair runs.
    pub enabled: bool,
    /// Spoken punctuation conversion.
    pub spoken: bool,
    /// Terminal punctuation insertion.
    pub ensure_terminal: bool,
}

impl Default for PunctuationSection {
    fn default() -> Self {
        Self {
            enabled: true,
            spoken: true,
            ensure_terminal: true,
        }
    }
}

/// One `[[cleanup.rules]]` entry.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSection {
    /// Regex pattern.
    pub pattern: String,
    /// Replacement text.
    pub replacement: String,
}

/// `[dictionaries]`.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DictionariesSection {
    /// Dictionary sets this profile subscribes to.
    pub sets: Vec<String>,
}

impl Default for DictionariesSection {
    fn default() -> Self {
        Self {
            sets: vec!["user".into()],
        }
    }
}

/// `[format]`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FormatSection {
    /// Casing commands ("camel case foo bar" → `fooBar`).
    pub casing_commands: bool,
    /// Email greeting/sign-off layout.
    pub email_layout: bool,
    /// "- " bullet per final segment.
    pub bullets: bool,
}

/// `[cloud]`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CloudSection {
    /// Hard deny for network rewriters when false (docs/06 T2). Default
    /// false: cloud is opt-in per profile, never opt-out.
    pub rewriter_allowed: bool,
}

impl CleanupSection {
    fn to_config(&self, format: &FormatSection) -> CleanupConfig {
        CleanupConfig {
            whitespace: self.whitespace.enabled,
            fillers: FillerConfig {
                enabled: self.fillers.enabled,
                extra: self.fillers.extra.clone(),
            },
            dictionary: self.dictionary.enabled,
            symbols: SymbolsConfig {
                enabled: self.symbols.enabled,
                table: self.symbols.table.clone(),
            },
            punctuation: PunctuationConfig {
                enabled: self.punctuation.enabled,
                spoken: self.punctuation.spoken,
                ensure_terminal: self.punctuation.ensure_terminal,
            },
            segmentation: self.segmentation.enabled,
            capitalization: self.capitalization.enabled,
            proper_nouns: self.proper_nouns.clone(),
            user_rules: self
                .rules
                .iter()
                .map(|r| UserRule {
                    pattern: r.pattern.clone(),
                    replacement: r.replacement.clone(),
                })
                .collect(),
            format: FormatConfig {
                casing_commands: format.casing_commands,
                email_layout: format.email_layout,
                bullets: format.bullets,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SqliteDictionaryRepo;
    use od_core_types::DictEntry;

    fn empty_repo() -> SqliteDictionaryRepo {
        SqliteDictionaryRepo::open_in_memory().unwrap()
    }

    #[test]
    fn all_shipped_profiles_parse_and_resolve() {
        let store = ProfileStore::new(std::env::temp_dir().join("od-does-not-exist"));
        let repo = empty_repo();
        for id in shipped_profile_ids() {
            let p = store.load(id).unwrap_or_else(|e| panic!("{id}: {e}"));
            let ctx = resolve_profile(&p, &repo).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(!ctx.profile.name.is_empty(), "{id} has a name");
        }
    }

    #[test]
    fn coding_profile_shape() {
        let store = ProfileStore::new(std::env::temp_dir().join("od-does-not-exist"));
        let p = store.load("coding").unwrap();
        let ctx = resolve_profile(&p, &empty_repo()).unwrap();
        let prof = &ctx.profile;
        assert_eq!(prof.name, "Coding");
        assert!(!prof.cleanup.capitalization, "code casing owns caps");
        assert_eq!(prof.cleanup.symbols.table, "programming");
        assert!(prof.cleanup.format.casing_commands);
        assert!(!prof.rewriter_allowed);
    }

    #[test]
    fn resolve_pulls_vocab_and_entries_from_repo() {
        let mut repo = empty_repo();
        repo.add(
            "user",
            &DictEntry {
                spoken: "open dictate".into(),
                written: "OpenDictate".into(),
                case_sensitive: false,
            },
        )
        .unwrap();
        let store = ProfileStore::new(std::env::temp_dir().join("od-does-not-exist"));
        let p = store.load("general").unwrap();
        let ctx = resolve_profile(&p, &repo).unwrap();
        assert_eq!(ctx.vocab.terms, vec!["OpenDictate".to_owned()]);
        assert_eq!(ctx.profile.entries.len(), 1);
    }

    #[test]
    fn user_profile_shadows_shipped_and_unknown_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("general.toml"),
            "[profile]\nname = \"Mine\"\n",
        )
        .unwrap();
        let store = ProfileStore::new(dir.path().to_path_buf());
        assert_eq!(store.load("general").unwrap().profile.name, "Mine");
        assert!(matches!(
            store.load("nope"),
            Err(StorageError::UnknownProfile(_))
        ));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let bad = "[profile]\nname = \"X\"\ntypo_key = 1\n";
        assert!(toml::from_str::<ProfileToml>(bad).is_err());
    }

    #[test]
    fn language_auto_resolves() {
        let p: ProfileToml =
            toml::from_str("[profile]\nname = \"X\"\n[stt]\nlanguage = \"auto\"\n").unwrap();
        let ctx = resolve_profile(&p, &empty_repo()).unwrap();
        assert_eq!(ctx.language, od_core_types::LanguageHint::Auto);
    }
}
