//! Serving classes (decided 2026-10-10): what an Oyzu proxy may do with a
//! tool's bytes. Unknown licenses are `direct`, never `cache`.
use crate::config::ServingEntry;
use crate::index::ServingClass;

/// A tool's serving class and why. The index carries only `class`; the rest
/// goes into the review summary.
#[derive(Debug, Clone, PartialEq)]
pub struct Serving {
    pub class: ServingClass,
    pub basis: String,
    pub license: Option<String>,
    pub source_mirror: bool,
}

/// Licenses that allow redistribution of unmodified copies with attribution
/// that travels inside the archive.
const PERMISSIVE: &[&str] = &[
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "MPL-2.0",
    "Unlicense",
    "0BSD",
    "Zlib",
    "PSF-2.0",
    "BSL-1.0",
];
/// Redistributable when the matching source is mirrored beside the binary.
const COPYLEFT: &[&str] = &[
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "GPL-2.0-only WITH Classpath-exception-2.0",
];
/// Redistribution granted, but an organization may want to block them.
const SOURCE_AVAILABLE: &[&str] = &["BUSL-1.1", "FSL-1.1-MIT", "FSL-1.1-ALv2", "Elastic-2.0"];

/// The class a license maps to, and whether the source must be mirrored.
/// `A OR B` takes the most permissive alternative; anything else unknown is
/// `direct`.
pub fn class_for_license(spdx: &str) -> (ServingClass, bool) {
    let mut best: Option<(ServingClass, bool)> = None;
    for alt in spdx.split(" OR ") {
        let alt = alt
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .trim();
        let c = if PERMISSIVE.contains(&alt) {
            (ServingClass::Cache, false)
        } else if COPYLEFT.contains(&alt) {
            (ServingClass::Cache, true)
        } else if SOURCE_AVAILABLE.contains(&alt) {
            (ServingClass::CacheSourceAvailable, false)
        } else {
            (ServingClass::Direct, false)
        };
        best = Some(match best {
            None => c,
            Some(b) => {
                // Prefer a class that needs no source mirror, then the lower class.
                if (c.0, c.1) < (b.0, b.1) {
                    c
                } else {
                    b
                }
            }
        });
    }
    best.unwrap_or((ServingClass::Direct, false))
}

fn parse_class(text: &str) -> ServingClass {
    match text {
        "cache" => ServingClass::Cache,
        "cache-source-available" => ServingClass::CacheSourceAvailable,
        "excluded" => ServingClass::Excluded,
        _ => ServingClass::Direct,
    }
}

/// Products whose license forbids redistribution by a proxy whatever a
/// supplement says: Oracle JDK, Oracle GraalVM (and, conservatively, any
/// GraalVM) and the Android SDK. A tool id naming one of them is never above
/// `direct`.
const NEVER_PROXIED: &[&str] = &["oracle", "graalvm", "android"];

/// Whether a tool id (`core:<provider>`, `aqua:<package>`) names a product
/// that is never proxied.
pub fn never_proxied(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    NEVER_PROXIED.iter().any(|name| id.contains(name))
}

/// The serving class for the tool `id`, from its supplement entry.
pub fn serving_for_tool(id: &str, entry: Option<&ServingEntry>) -> Serving {
    let mut serving = serving_for(entry);
    if never_proxied(id) && !matches!(serving.class, ServingClass::Direct | ServingClass::Excluded)
    {
        serving.class = ServingClass::Direct;
        serving.basis = "never proxied".into();
        serving.source_mirror = false;
    }
    serving
}

pub fn serving_for(entry: Option<&ServingEntry>) -> Serving {
    match entry {
        None => Serving {
            class: ServingClass::Direct,
            basis: "default".into(),
            license: None,
            source_mirror: false,
        },
        Some(e) if !e.reviewed => Serving {
            // A proposal is shown, never acted on, except that a proposal to
            // exclude is honored: being too careful costs nothing here.
            class: if e.class.as_deref() == Some("excluded") {
                ServingClass::Excluded
            } else {
                ServingClass::Direct
            },
            basis: "unreviewed".into(),
            license: Some(e.license.clone()),
            source_mirror: false,
        },
        Some(e) => {
            let (mapped, mirror) = class_for_license(&e.license);
            let class = e.class.as_deref().map(parse_class).unwrap_or(mapped);
            Serving {
                class,
                basis: "supplement".into(),
                license: Some(e.license.clone()),
                source_mirror: mirror && class != ServingClass::Direct,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(license: &str, class: Option<&str>, reviewed: bool) -> ServingEntry {
        ServingEntry {
            license: license.into(),
            class: class.map(str::to_owned),
            reviewed,
            note: None,
        }
    }

    #[test]
    fn licenses_map_to_classes() {
        assert_eq!(class_for_license("MIT"), (ServingClass::Cache, false));
        assert_eq!(
            class_for_license("AGPL-3.0-only"),
            (ServingClass::Cache, true)
        );
        assert_eq!(
            class_for_license("BUSL-1.1"),
            (ServingClass::CacheSourceAvailable, false)
        );
        assert_eq!(
            class_for_license("MIT OR Apache-2.0"),
            (ServingClass::Cache, false)
        );
        assert_eq!(
            class_for_license("GPL-3.0-only OR MIT"),
            (ServingClass::Cache, false)
        );
        assert_eq!(
            class_for_license("LicenseRef-proprietary"),
            (ServingClass::Direct, false)
        );
    }

    #[test]
    fn unknown_and_unreviewed_tools_stay_direct() {
        assert_eq!(serving_for(None).class, ServingClass::Direct);
        let s = serving_for(Some(&entry("MIT", None, false)));
        assert_eq!(
            (s.class, s.basis.as_str()),
            (ServingClass::Direct, "unreviewed")
        );
        let s = serving_for(Some(&entry("MIT", Some("excluded"), false)));
        assert_eq!(s.class, ServingClass::Excluded);
    }

    #[test]
    fn oracle_graalvm_and_android_are_never_proxied() {
        let cache = entry("Apache-2.0", Some("cache"), true);
        for id in [
            "core:oracle-jdk",
            "aqua:graalvm/graalvm-ce-builds",
            "core:android-sdk",
            "aqua:Oracle/Something",
        ] {
            let s = serving_for_tool(id, Some(&cache));
            assert_eq!(
                (s.class, s.basis.as_str()),
                (ServingClass::Direct, "never proxied"),
                "{id}"
            );
        }
        let excluded = entry("LicenseRef-Oracle", Some("excluded"), true);
        assert_eq!(
            serving_for_tool("core:oracle-jdk", Some(&excluded)).class,
            ServingClass::Excluded
        );
        // Temurin is GPLv2 with the Classpath Exception: cached with its source
        // mirrored once a person reviews it, direct until then.
        let temurin = entry("GPL-2.0-only WITH Classpath-exception-2.0", None, true);
        let s = serving_for_tool("core:java", Some(&temurin));
        assert_eq!((s.class, s.source_mirror), (ServingClass::Cache, true));
        let s = serving_for_tool(
            "core:java",
            Some(&entry(
                "GPL-2.0-only WITH Classpath-exception-2.0",
                None,
                false,
            )),
        );
        assert_eq!(s.class, ServingClass::Direct);
    }

    #[test]
    fn reviewed_entries_apply_and_overrides_win() {
        let s = serving_for(Some(&entry("GPL-3.0-only", None, true)));
        assert_eq!((s.class, s.source_mirror), (ServingClass::Cache, true));
        let s = serving_for(Some(&entry("MIT", Some("direct"), true)));
        assert_eq!(s.class, ServingClass::Direct);
    }
}
