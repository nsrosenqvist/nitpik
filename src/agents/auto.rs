//! Auto-selection: routing a diff to review lenses.
//!
//! The default engine selects issue-typed *lenses* by diff substance. This
//! module provides both selection paths:
//!
//! - [`heuristic_lens_candidates`] — the key-free path: file/path
//!   classification ([`classify_files`], [`should_include_architect`]) routes
//!   coarse signals to conditional lenses (frontend → `a11y`/`user-journey`,
//!   tests → `test-integrity`, structural → `operational`/`contract-impact`/
//!   `holistic`, docs → `docs-drift`).
//! - [`build_lens_triage_summary`] + [`parse_triage_lenses`] — the LLM path:
//!   present the candidate menu to the `triage` profile and interpret its pick.
//!
//! The always-on lenses (security, correctness) are added separately by
//! [`crate::agents::list_always_include_profiles`]. See
//! `plans/pr-native-review/lens-model.md`.

use crate::models::diff::FileDiff;

/// Backend-indicating path segments for JS/TS files.
const JS_BACKEND_PATH_SEGMENTS: &[&str] = &[
    "controllers/",
    "middleware/",
    "routes/",
    "handlers/",
    "resolvers/",
    "services/",
    "repositories/",
    "migrations/",
    "seeds/",
    "prisma/",
    "graphql/",
    "db/",
    "schemas/",
    "trpc/",
    "lambdas/",
    "functions/",
];

/// Backend-indicating filename suffixes for JS/TS files (NestJS, etc.).
const JS_BACKEND_FILE_SUFFIXES: &[&str] = &[
    ".controller.ts",
    ".controller.js",
    ".service.ts",
    ".service.js",
    ".middleware.ts",
    ".middleware.js",
    ".resolver.ts",
    ".resolver.js",
    ".module.ts",
    ".guard.ts",
    ".guard.js",
    ".interceptor.ts",
    ".interceptor.js",
    ".pipe.ts",
    ".pipe.js",
    ".gateway.ts",
    ".gateway.js",
    ".entity.ts",
    ".entity.js",
    ".dto.ts",
    ".dto.js",
    ".repository.ts",
    ".repository.js",
];

/// Backend-indicating root filenames (exact matches) for JS/TS files.
const JS_BACKEND_ROOT_FILES: &[&str] = &[
    "server.ts",
    "server.js",
    "server.mts",
    "server.mjs",
    "app.ts",
    "app.js",
    "index.ts", // root index in a non-frontend project often is a server
    "index.js",
];

/// Frontend-indicating path segments for JS/TS files.
const JS_FRONTEND_PATH_SEGMENTS: &[&str] = &[
    "components/",
    "pages/",
    "views/",
    "public/",
    "static/",
    "styles/",
    "hooks/",
    "stores/",
    "layouts/",
    "composables/",
    "assets/",
    "app/", // Next.js app router
];

/// Files and path patterns that indicate cross-cutting / architectural changes.
/// A diff touching these warrants architectural review.
const ARCHITECTURE_FILE_PATTERNS: &[&str] = &[
    // CI / CD
    ".github/workflows/",
    ".gitlab-ci.yml",
    ".circleci/",
    "Jenkinsfile",
    ".buildkite/",
    ".travis.yml",
    "azure-pipelines.yml",
    "bitbucket-pipelines.yml",
    // Containerization / orchestration
    "Dockerfile",
    "docker-compose",
    "compose.yml",
    "compose.yaml",
    "kubernetes/",
    "k8s/",
    "helm/",
    ".dockerignore",
    // Infrastructure as Code
    "terraform/",
    ".tf",
    "pulumi/",
    "cdk.json",
    "serverless.yml",
    "serverless.ts",
    "wrangler.toml",
    "cloudformation/",
    "sam.yml",
    "sam.yaml",
    // Build system / project config
    "Makefile",
    "CMakeLists.txt",
    "build.gradle",
    "pom.xml",
    "build.rs",
    "build.zig",
    ".cargo/config",
    "nx.json",
    "turbo.json",
    "lerna.json",
    "pnpm-workspace.yaml",
    // Dependency manifests (changing deps can signal architectural shifts)
    "Cargo.toml",
    "package.json",
    "go.mod",
    "requirements.txt",
    "pyproject.toml",
    "Gemfile",
    "build.sbt",
    "deps.edn",
    // Database migrations
    "migrations/",
    "alembic/",
    // API definitions
    "openapi",
    "swagger",
    ".proto",
    ".graphql",
    ".gql",
];

/// Minimum number of changed files to consider a diff "large" enough for
/// architectural review, even without explicit architectural file signals.
const LARGE_DIFF_FILE_THRESHOLD: usize = 15;

/// Minimum number of distinct directories touched to consider a diff
/// structurally broad enough for architectural review.
const BROAD_DIFF_DIR_THRESHOLD: usize = 8;

/// Select built-in reviewer profiles based on the changed files and the
/// repository root.
///
/// See the [module-level documentation](self) for the full decision tree.
///
/// # Arguments
///
/// * `diffs` — The set of file diffs to classify.
/// * `repo_root` — Path to the repository root, used to read `package.json`
///   and check for backend config files when JS/TS path signals are
///   ambiguous.
///
/// # Returns
///
/// A non-empty list of profile name strings (e.g. `["frontend", "backend"]`)
/// suitable for passing to [`crate::agents::resolve_profiles`].
/// Accumulated classification signals from analyzing changed files.
struct FileClassification {
    has_frontend: bool,
    has_backend: bool,
    has_js_ts: bool,
    js_ts_backend_signals: u32,
    js_ts_frontend_signals: u32,
}

/// Classify diff files by extension, path patterns, and filename heuristics.
fn classify_files(diffs: &[FileDiff<'_>]) -> FileClassification {
    let mut c = FileClassification {
        has_frontend: false,
        has_backend: false,
        has_js_ts: false,
        js_ts_backend_signals: 0,
        js_ts_frontend_signals: 0,
    };

    for diff in diffs {
        let path = diff.path();
        let ext = path.rsplit('.').next().unwrap_or("");

        // ── Always-frontend extensions ────────────────────────────────
        match ext {
            "vue" | "svelte" | "css" | "scss" | "less" | "html" | "astro" => {
                c.has_frontend = true;
                continue;
            }
            _ => {}
        }

        // ── Always-backend extensions ─────────────────────────────────
        match ext {
            "rs" | "go" | "py" | "rb" | "java" | "kt" | "cs" | "php" | "ex" | "exs" | "c"
            | "cpp" | "h" | "hpp" | "scala" | "clj" | "zig" | "nim" | "erl" | "gleam" => {
                c.has_backend = true;
                continue;
            }
            _ => {}
        }

        // ── Ambiguous JS/TS extensions — classify by context ──────────
        if matches!(
            ext,
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "mts" | "cjs" | "cts"
        ) {
            c.has_js_ts = true;

            if JS_BACKEND_FILE_SUFFIXES.iter().any(|s| path.ends_with(s)) {
                c.js_ts_backend_signals += 1;
                continue;
            }

            let filename = path.rsplit('/').next().unwrap_or(path);
            if JS_BACKEND_ROOT_FILES.contains(&filename) && !is_frontend_path(path) {
                c.js_ts_backend_signals += 1;
                continue;
            }

            if JS_BACKEND_PATH_SEGMENTS
                .iter()
                .any(|seg| path.contains(seg))
            {
                c.js_ts_backend_signals += 1;
                continue;
            }
            if JS_FRONTEND_PATH_SEGMENTS
                .iter()
                .any(|seg| path.contains(seg))
            {
                c.js_ts_frontend_signals += 1;
                continue;
            }

            if matches!(ext, "jsx" | "tsx") {
                c.js_ts_frontend_signals += 1;
            }

            continue;
        }

        // ── Generic path-based heuristics for other extensions ────────
        if path.contains("frontend/") || path.contains("client/") {
            c.has_frontend = true;
        }
        if path.contains("backend/") || path.contains("server/") || path.contains("api/") {
            c.has_backend = true;
        }
    }

    c
}

/// Returns `true` if the diff set warrants an architect reviewer.
///
/// Triggers on structural files (CI, IaC, build configs) or large/broad diffs.
fn should_include_architect(diffs: &[FileDiff<'_>]) -> bool {
    let touches_architecture = diffs.iter().any(|d| {
        let p = d.path();
        ARCHITECTURE_FILE_PATTERNS.iter().any(|pat| p.contains(pat))
    });

    let file_count = diffs.len();
    let dir_count = {
        let mut dirs: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for d in diffs {
            if let Some(parent) = d.path().rsplit_once('/').map(|(dir, _)| dir) {
                dirs.insert(parent);
            }
        }
        dirs.len()
    };
    let is_large_diff =
        file_count >= LARGE_DIFF_FILE_THRESHOLD || dir_count >= BROAD_DIFF_DIR_THRESHOLD;

    touches_architecture || is_large_diff
}

/// Strategy for selecting reviewer profiles when `--profile auto` is used.
///
/// The CLI parses this via clap's `ValueEnum`; serialization yields the
/// kebab-case form used both on the command line and in the audit log
/// `ConfigSummary`, keeping a single source of truth for the spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoMode {
    /// Pure file/path/dependency heuristics (no LLM call).
    Heuristic,
    /// Always call the LLM to pick profiles.
    Llm,
    /// Heuristics first; consult the LLM only when heuristics are
    /// inconclusive (default).
    Hybrid,
}

impl std::fmt::Display for AutoMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AutoMode::Heuristic => "heuristic",
            AutoMode::Llm => "llm",
            AutoMode::Hybrid => "hybrid",
        })
    }
}

/// Build a compact diff summary for the LLM triage profile.
///
/// Lists each changed file path on its own line with a small marker for
/// added/modified/removed lines. The summary is capped to keep token
/// use predictable on very large diffs.
pub fn build_triage_summary(diffs: &[FileDiff<'_>]) -> String {
    const MAX_FILES: usize = 200;
    let mut s = String::with_capacity(diffs.len().min(MAX_FILES) * 80);
    s.push_str("## Changed files\n\n");
    for d in diffs.iter().take(MAX_FILES) {
        let mut added = 0usize;
        let mut removed = 0usize;
        for h in &d.hunks {
            for line in &h.lines {
                match line.line_type {
                    crate::models::diff::DiffLineType::Added => added += 1,
                    crate::models::diff::DiffLineType::Removed => removed += 1,
                    _ => {}
                }
            }
        }
        s.push_str(&format!("- `{}` (+{added}/-{removed})\n", d.path()));
    }
    if diffs.len() > MAX_FILES {
        s.push_str(&format!("- … and {} more files\n", diffs.len() - MAX_FILES));
    }
    s.push_str(
        "\nPick the smallest set of reviewer profiles needed. Return only the JSON array.\n",
    );
    s
}

/// Heuristic conditional-lens candidates for the **key-free** path (no LLM
/// triage available, or `--auto heuristic`).
///
/// Reuses the file classification to route coarse signals to lenses:
/// frontend → `a11y` + `user-journey`; backend → `concurrency` +
/// `performance`; structural/large → `operational` + `contract-impact` +
/// `holistic`; test files → `test-integrity`; docs → `docs-drift`. This is a
/// deliberately coarse fallback — the LLM triage (the default) does
/// substance-based selection. The caller intersects the result with the
/// lenses actually available so a removed/overridden lens can't slip in.
///
/// The always-on lenses (security, correctness) are added separately and are
/// not returned here.
pub fn heuristic_lens_candidates(diffs: &[FileDiff<'_>]) -> Vec<String> {
    let c = classify_files(diffs);
    let frontend = c.has_frontend || c.js_ts_frontend_signals > 0;
    let backend = c.has_backend || c.js_ts_backend_signals > 0;
    let structural = should_include_architect(diffs);
    let has_tests = diffs.iter().any(|d| is_test_path(d.path()));
    let has_docs = diffs.iter().any(|d| is_docs_path(d.path()));

    let mut out: Vec<String> = Vec::new();
    let push = |name: &str, out: &mut Vec<String>| {
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    };
    if frontend {
        push("a11y", &mut out);
        push("user-journey", &mut out);
    }
    if backend {
        push("concurrency", &mut out);
        push("performance", &mut out);
    }
    if structural {
        push("operational", &mut out);
        push("contract-impact", &mut out);
        push("holistic", &mut out);
    }
    if has_tests {
        push("test-integrity", &mut out);
    }
    if has_docs {
        push("docs-drift", &mut out);
    }
    out
}

/// Returns `true` if the path looks like a test file.
fn is_test_path(path: &str) -> bool {
    let p = path.to_lowercase();
    p.contains("/test")
        || p.starts_with("test")
        || p.contains("/spec")
        || p.contains("__tests__")
        || p.contains(".test.")
        || p.contains(".spec.")
        || p.contains("_test.")
        || p.ends_with("_test.go")
        || p.contains("/tests/")
}

/// Returns `true` if the path looks like documentation.
fn is_docs_path(path: &str) -> bool {
    let p = path.to_lowercase();
    p.ends_with(".md")
        || p.ends_with(".mdx")
        || p.ends_with(".rst")
        || p.contains("/docs/")
        || p.starts_with("docs/")
        || p.contains("readme")
        || p.contains("changelog")
        || p.contains("openapi")
        || p.contains("swagger")
}

/// Build the triage prompt body for **lens** selection: the changed-file
/// summary plus the menu of candidate lenses (name + one-line description)
/// the model may choose from.
pub fn build_lens_triage_summary(
    diffs: &[FileDiff<'_>],
    candidates: &[(String, String)],
) -> String {
    let mut s = build_triage_summary(diffs);
    // build_triage_summary ends with a profile-oriented instruction; replace
    // the tail with a lens menu + lens instruction.
    if let Some(pos) = s.find("\nPick the smallest set") {
        s.truncate(pos);
    }
    s.push_str("\n## Candidate lenses\n\n");
    for (name, desc) in candidates {
        s.push_str(&format!("- `{name}` — {desc}\n"));
    }
    s.push_str(
        "\nThe `security` and `correctness` lenses always run — do NOT list them. \
         From the candidates above, pick **only** those whose failure mode this \
         change plausibly contains. It is correct to pick none when the change is \
         small or none apply. Do not pad the list. Return only the JSON array.\n",
    );
    s
}

/// Parse triage verdicts into lens names, validated against `allowed`.
///
/// A classification not in `allowed` (a hallucinated name, or an always-on
/// lens the triage shouldn't pick) is dropped. Returns names in emitted
/// order, deduplicated.
pub fn parse_triage_lenses(
    verdicts: &[crate::providers::TriageVerdict],
    allowed: &[String],
) -> Vec<String> {
    let allowed_lower: std::collections::HashSet<String> =
        allowed.iter().map(|a| a.to_lowercase()).collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for v in verdicts {
        let name = v.classification.trim().to_lowercase();
        if !allowed_lower.contains(&name) {
            continue;
        }
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

/// Returns `true` if the path looks like it sits inside a frontend directory.
fn is_frontend_path(path: &str) -> bool {
    JS_FRONTEND_PATH_SEGMENTS
        .iter()
        .any(|seg| path.contains(seg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::FileDiff;

    fn make_diff(path: &str) -> FileDiff<'static> {
        FileDiff {
            old_path: path.to_string(),
            new_path: path.to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![],
        }
    }

    /// Returns a temp dir with no project root files (bare repo root).

    #[test]
    fn triage_summary_lists_files_with_counts() {
        let mut d = make_diff("src/foo.rs");
        d.hunks = vec![crate::models::diff::Hunk {
            old_start: 1,
            old_count: 0,
            new_start: 1,
            new_count: 2,
            header: None,
            lines: vec![
                crate::models::diff::DiffLine {
                    line_type: crate::models::diff::DiffLineType::Added,
                    content: "a".into(),
                    old_line_no: None,
                    new_line_no: Some(1),
                },
                crate::models::diff::DiffLine {
                    line_type: crate::models::diff::DiffLineType::Added,
                    content: "b".into(),
                    old_line_no: None,
                    new_line_no: Some(2),
                },
            ],
        }];
        let summary = build_triage_summary(&[d]);
        assert!(summary.contains("src/foo.rs"));
        assert!(summary.contains("+2/-0"));
    }

    #[test]
    fn heuristic_lens_candidates_routes_frontend() {
        let diffs = vec![make_diff("src/components/Button.tsx")];
        let lenses = heuristic_lens_candidates(&diffs);
        assert!(lenses.contains(&"a11y".to_string()), "got: {lenses:?}");
        assert!(lenses.contains(&"user-journey".to_string()));
    }

    #[test]
    fn heuristic_lens_candidates_routes_tests_and_docs() {
        let diffs = vec![make_diff("src/foo_test.rs"), make_diff("docs/guide.md")];
        let lenses = heuristic_lens_candidates(&diffs);
        assert!(
            lenses.contains(&"test-integrity".to_string()),
            "got: {lenses:?}"
        );
        assert!(lenses.contains(&"docs-drift".to_string()));
    }

    #[test]
    fn heuristic_lens_candidates_structural_change() {
        let diffs = vec![make_diff("Dockerfile")];
        let lenses = heuristic_lens_candidates(&diffs);
        assert!(
            lenses.contains(&"operational".to_string()),
            "got: {lenses:?}"
        );
        assert!(lenses.contains(&"contract-impact".to_string()));
        assert!(lenses.contains(&"holistic".to_string()));
    }

    #[test]
    fn parse_triage_lenses_filters_to_allowed() {
        use crate::providers::TriageVerdict;
        let v = vec![
            TriageVerdict {
                index: 0,
                classification: "concurrency".into(),
                rationale: None,
            },
            TriageVerdict {
                index: 1,
                classification: "security".into(), // always-on, not a candidate
                rationale: None,
            },
            TriageVerdict {
                index: 2,
                classification: "made-up".into(),
                rationale: None,
            },
        ];
        let allowed = vec!["concurrency".to_string(), "performance".to_string()];
        assert_eq!(
            parse_triage_lenses(&v, &allowed),
            vec!["concurrency".to_string()]
        );
    }

    #[test]
    fn build_lens_triage_summary_lists_candidate_menu() {
        let diffs = vec![make_diff("src/foo.rs")];
        let menu = vec![
            (
                "concurrency".to_string(),
                "races and shared state".to_string(),
            ),
            ("performance".to_string(), "hot-path cost".to_string()),
        ];
        let s = build_lens_triage_summary(&diffs, &menu);
        assert!(s.contains("Candidate lenses"));
        assert!(s.contains("`concurrency` — races and shared state"));
        assert!(s.contains("do NOT list them"));
        // The profile-oriented tail of build_triage_summary is replaced.
        assert!(!s.contains("reviewer profiles needed"));
    }
}
