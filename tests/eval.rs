//! Review-quality evaluation harness.
//!
//! Scores nitpik's review output against a labeled corpus so prompt,
//! dedup, scope, and verify changes can be measured rather than guessed.
//! The corpus lives in `tests/fixtures/eval/` — see that directory's
//! `README.md` for its shape and how to add cases.
//!
//! Two kinds of cases:
//! - **positive** — a fixture with one or more *planted* issues at known
//!   lines. Measures **recall**: did the reviewer catch them?
//! - **negative** — fixture code that is fine but tempting to flag
//!   (cosmetic edits, renames, a load-bearing guard). Measures **noise**:
//!   any `warning`/`error` finding here is a false positive.
//!
//! # Running
//!
//! The scoring logic is unit-tested deterministically under a normal
//! `cargo test`. The end-to-end scorecard makes real LLM calls and is
//! `#[ignore]`d; run it explicitly with an API key:
//!
//! ```sh
//! ANTHROPIC_API_KEY=sk-... cargo test --test eval -- --ignored --nocapture
//! ```
//!
//! Set `NITPIK_EVAL_BASELINE=path.json` to also write the scorecard to a
//! file (for committing a baseline / diffing in CI).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nitpik::models::finding::{Finding, Severity};
use serde::Deserialize;

// ===========================================================================
// Corpus label schema
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CaseKind {
    /// Fixture contains planted issue(s) the reviewer should catch.
    Positive,
    /// Fixture is clean; any warning/error finding is noise.
    Negative,
}

/// One expected (ground-truth) finding in a positive fixture.
#[derive(Debug, Clone, Deserialize)]
struct Label {
    file: String,
    line: u32,
    /// Lowest severity that counts as catching this issue. Defaults to
    /// `warning` so an `info`-level mention doesn't count as a catch.
    #[serde(default)]
    min_severity: Option<Severity>,
    #[serde(default)]
    #[allow(dead_code)]
    note: String,
}

/// Parsed `expected.json` for a fixture case.
#[derive(Debug, Clone, Deserialize)]
struct ExpectedCase {
    #[allow(dead_code)]
    description: String,
    kind: CaseKind,
    profiles: Vec<String>,
    #[serde(default)]
    expected: Vec<Label>,
}

// ===========================================================================
// Scoring core (pure — unit-tested without a provider)
// ===========================================================================

/// How far a produced finding's line may sit from a label's line and
/// still count as the same issue. LLMs frequently anchor a line or two
/// off (e.g. on the function header vs the buggy statement).
const LINE_TOLERANCE: u32 = 2;

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Info => 0,
        Severity::Warning => 1,
        Severity::Error => 2,
    }
}

/// A produced finding matches a label when it is on the same file, within
/// [`LINE_TOLERANCE`] lines, and at least as severe as the label requires.
fn finding_matches_label(finding: &Finding, label: &Label) -> bool {
    let same_file = finding.file == label.file || Path::new(&finding.file).ends_with(&label.file);
    if !same_file {
        return false;
    }
    if finding.line.abs_diff(label.line) > LINE_TOLERANCE {
        return false;
    }
    let min = label.min_severity.unwrap_or(Severity::Warning);
    severity_rank(finding.severity) >= severity_rank(min)
}

/// The score for a single case.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CaseScore {
    name: String,
    kind: CaseKind,
    /// Labels matched by ≥1 finding (positives).
    caught: usize,
    /// Total labels (positives).
    total_labels: usize,
    /// Findings on a positive case matching no label — surfaced, not
    /// penalized (a positive fixture may contain other real issues).
    extra: usize,
    /// Warning+ findings on a negative case — unambiguous noise.
    noise: usize,
}

fn score_case(name: &str, case: &ExpectedCase, findings: &[Finding]) -> CaseScore {
    match case.kind {
        CaseKind::Positive => {
            let caught = case
                .expected
                .iter()
                .filter(|label| findings.iter().any(|f| finding_matches_label(f, label)))
                .count();
            let extra = findings
                .iter()
                .filter(|f| !case.expected.iter().any(|l| finding_matches_label(f, l)))
                .count();
            CaseScore {
                name: name.to_string(),
                kind: case.kind,
                caught,
                total_labels: case.expected.len(),
                extra,
                noise: 0,
            }
        }
        CaseKind::Negative => {
            let noise = findings
                .iter()
                .filter(|f| severity_rank(f.severity) >= severity_rank(Severity::Warning))
                .count();
            CaseScore {
                name: name.to_string(),
                kind: case.kind,
                caught: 0,
                total_labels: 0,
                extra: 0,
                noise,
            }
        }
    }
}

/// Aggregate scorecard across all cases.
#[derive(Debug, Clone)]
struct Scorecard {
    cases: Vec<CaseScore>,
}

impl Scorecard {
    fn labels_total(&self) -> usize {
        self.cases.iter().map(|c| c.total_labels).sum()
    }
    fn labels_caught(&self) -> usize {
        self.cases.iter().map(|c| c.caught).sum()
    }
    fn recall(&self) -> f64 {
        let total = self.labels_total();
        if total == 0 {
            return 1.0;
        }
        self.labels_caught() as f64 / total as f64
    }
    fn negatives(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.kind == CaseKind::Negative)
            .count()
    }
    fn clean_negatives(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.kind == CaseKind::Negative && c.noise == 0)
            .count()
    }
    fn total_noise(&self) -> usize {
        self.cases.iter().map(|c| c.noise).sum()
    }

    /// Human-readable scorecard table.
    fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "\n=== nitpik eval scorecard ===\n");
        for c in &self.cases {
            match c.kind {
                CaseKind::Positive => {
                    let _ = writeln!(
                        s,
                        "  [pos] {:<24} caught {}/{}  (+{} extra)",
                        c.name, c.caught, c.total_labels, c.extra
                    );
                }
                CaseKind::Negative => {
                    let mark = if c.noise == 0 { "clean" } else { "NOISE" };
                    let _ = writeln!(
                        s,
                        "  [neg] {:<24} {} ({} warning+ findings)",
                        c.name, mark, c.noise
                    );
                }
            }
        }
        let _ = writeln!(
            s,
            "\n  recall:          {:.0}% ({}/{} planted issues caught)",
            self.recall() * 100.0,
            self.labels_caught(),
            self.labels_total()
        );
        let _ = writeln!(
            s,
            "  negative clean:  {}/{} ({} total noise findings)",
            self.clean_negatives(),
            self.negatives(),
            self.total_noise()
        );
        s
    }

    /// Machine-readable summary for baselines / CI diffing.
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "recall": self.recall(),
            "labels_caught": self.labels_caught(),
            "labels_total": self.labels_total(),
            "negatives_clean": self.clean_negatives(),
            "negatives_total": self.negatives(),
            "total_noise": self.total_noise(),
            "cases": self.cases.iter().map(|c| serde_json::json!({
                "name": c.name,
                "kind": format!("{:?}", c.kind).to_lowercase(),
                "caught": c.caught,
                "total_labels": c.total_labels,
                "extra": c.extra,
                "noise": c.noise,
            })).collect::<Vec<_>>(),
        })
    }
}

// ===========================================================================
// Deterministic unit tests for the scoring core (no provider, no API key)
// ===========================================================================

#[cfg(test)]
mod scoring_tests {
    use super::*;

    fn finding(file: &str, line: u32, sev: Severity) -> Finding {
        Finding {
            file: file.to_string(),
            line,
            end_line: None,
            severity: sev,
            title: "t".into(),
            message: "m".into(),
            suggestion: None,
            agent: "backend".into(),
            evidence: Vec::new(),
        }
    }

    fn label(file: &str, line: u32, min: Severity) -> Label {
        Label {
            file: file.into(),
            line,
            min_severity: Some(min),
            note: String::new(),
        }
    }

    fn positive(labels: Vec<Label>) -> ExpectedCase {
        ExpectedCase {
            description: String::new(),
            kind: CaseKind::Positive,
            profiles: vec!["backend".into()],
            expected: labels,
        }
    }

    fn negative() -> ExpectedCase {
        ExpectedCase {
            description: String::new(),
            kind: CaseKind::Negative,
            profiles: vec!["backend".into()],
            expected: vec![],
        }
    }

    #[test]
    fn match_within_tolerance_and_severity() {
        let l = label("a.py", 10, Severity::Warning);
        assert!(finding_matches_label(
            &finding("a.py", 12, Severity::Warning),
            &l
        )); // +2 ok
        assert!(finding_matches_label(
            &finding("a.py", 8, Severity::Error),
            &l
        )); // higher sev ok
        assert!(!finding_matches_label(
            &finding("a.py", 13, Severity::Error),
            &l
        )); // too far
        assert!(!finding_matches_label(
            &finding("a.py", 10, Severity::Info),
            &l
        )); // too mild
        assert!(!finding_matches_label(
            &finding("b.py", 10, Severity::Error),
            &l
        )); // wrong file
    }

    #[test]
    fn match_accepts_repo_relative_suffix() {
        // Finding paths may be repo-relative subpaths; a label naming the
        // bare file should still match.
        let l = label("repo.py", 2, Severity::Error);
        assert!(finding_matches_label(
            &finding("src/repo.py", 2, Severity::Error),
            &l
        ));
    }

    #[test]
    fn positive_case_counts_caught_and_extra() {
        let case = positive(vec![label("a.py", 5, Severity::Warning)]);
        let findings = vec![
            finding("a.py", 5, Severity::Error), // catches the label
            finding("a.py", 40, Severity::Info), // unrelated extra
        ];
        let s = score_case("c", &case, &findings);
        assert_eq!(s.caught, 1);
        assert_eq!(s.total_labels, 1);
        assert_eq!(s.extra, 1);
        assert_eq!(s.noise, 0);
    }

    #[test]
    fn positive_case_missed_label() {
        let case = positive(vec![label("a.py", 5, Severity::Error)]);
        // Only an info-level mention — does not meet the label's bar.
        let s = score_case("c", &case, &[finding("a.py", 5, Severity::Info)]);
        assert_eq!(s.caught, 0);
        assert_eq!(s.extra, 1);
    }

    #[test]
    fn negative_case_counts_warning_plus_as_noise() {
        let case = negative();
        let findings = vec![
            finding("a.py", 1, Severity::Info), // tolerated on negatives
            finding("a.py", 2, Severity::Warning),
            finding("a.py", 3, Severity::Error),
        ];
        let s = score_case("c", &case, &findings);
        assert_eq!(s.noise, 2);
    }

    #[test]
    fn scorecard_aggregates() {
        let card = Scorecard {
            cases: vec![
                score_case(
                    "p1",
                    &positive(vec![label("a.py", 1, Severity::Warning)]),
                    &[finding("a.py", 1, Severity::Warning)],
                ),
                score_case(
                    "p2",
                    &positive(vec![label("b.py", 1, Severity::Error)]),
                    &[], // missed
                ),
                score_case("n1", &negative(), &[]), // clean
                score_case("n2", &negative(), &[finding("c.py", 1, Severity::Error)]), // noisy
            ],
        };
        assert_eq!(card.labels_total(), 2);
        assert_eq!(card.labels_caught(), 1);
        assert!((card.recall() - 0.5).abs() < 1e-9);
        assert_eq!(card.negatives(), 2);
        assert_eq!(card.clean_negatives(), 1);
        assert_eq!(card.total_noise(), 1);
    }
}

// ===========================================================================
// End-to-end scorecard (real LLM — #[ignore]d)
// ===========================================================================

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("eval")
}

fn has_api_key() -> bool {
    let config = nitpik::config::Config::load(None, &nitpik::env::Env::real()).unwrap_or_default();
    if config.provider.api_key.is_none() {
        eprintln!(
            "SKIPPING eval: no API key for provider '{}'. Set ANTHROPIC_API_KEY (or NITPIK_API_KEY).",
            config.provider.name
        );
        return false;
    }
    true
}

async fn run_git(repo_dir: &Path, args: &[&str]) {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()
        .await
        .unwrap_or_else(|e| panic!("git {}: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn copy_tree(src: &Path, dst: &Path) {
    for entry in walkdir::WalkDir::new(src)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let rel = entry.path().strip_prefix(src).unwrap();
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).ok();
        } else {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p).ok();
            }
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Build a temp git repo: commit `base/`, then overlay `changeset/` so the
/// working tree holds the diff to review.
async fn setup_repo(case_dir: &Path) -> (PathBuf, tempfile::TempDir) {
    let tmp = tempfile::Builder::new()
        .prefix("nitpik-eval-")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let repo = tmp.path().to_path_buf();

    run_git(&repo, &["init"]).await;
    run_git(&repo, &["config", "user.email", "eval@nitpik.dev"]).await;
    run_git(&repo, &["config", "user.name", "Nitpik Eval"]).await;

    copy_tree(&case_dir.join("base"), &repo);
    run_git(&repo, &["add", "."]).await;
    run_git(&repo, &["commit", "-m", "base"]).await;

    copy_tree(&case_dir.join("changeset"), &repo);
    (repo, tmp)
}

async fn diffs_owned(repo: &Path) -> Vec<nitpik::models::diff::FileDiff<'static>> {
    let input = nitpik::models::InputMode::GitBase("HEAD".to_string());
    let source = nitpik::diff::get_diff_source(&input, repo)
        .await
        .expect("get diff source");
    match source {
        nitpik::diff::DiffSource::Raw(content) => {
            nitpik::diff::parser::parse_unified_diff(&content)
                .into_iter()
                .map(|d| nitpik::models::diff::FileDiff {
                    old_path: d.old_path,
                    new_path: d.new_path,
                    is_new: d.is_new,
                    is_deleted: d.is_deleted,
                    is_rename: d.is_rename,
                    is_binary: d.is_binary,
                    hunks: d
                        .hunks
                        .into_iter()
                        .map(|h| nitpik::models::diff::Hunk {
                            old_start: h.old_start,
                            old_count: h.old_count,
                            new_start: h.new_start,
                            new_count: h.new_count,
                            header: h.header,
                            lines: h
                                .lines
                                .into_iter()
                                .map(|l| nitpik::models::diff::DiffLine {
                                    line_type: l.line_type,
                                    content: std::borrow::Cow::Owned(l.content.into_owned()),
                                    old_line_no: l.old_line_no,
                                    new_line_no: l.new_line_no,
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect()
        }
        nitpik::diff::DiffSource::Scanned(diffs) => diffs,
    }
}

/// Run the full review pipeline (with the verify pass on, since the eval
/// is meant to reflect what ships) and return the findings.
async fn review_case(
    repo: &Path,
    profiles: &[String],
    config: &nitpik::config::Config,
) -> Vec<Finding> {
    let diffs = diffs_owned(repo).await;
    assert!(
        !diffs.is_empty(),
        "changeset produced no diff in {}",
        repo.display()
    );

    let baseline =
        nitpik::context::build_baseline_context(repo, &diffs, config, false, &[], Vec::new(), None)
            .await;

    // Full DI: construct the provider at the composition root (this harness)
    // and inject it into the engine.
    let provider: Arc<dyn nitpik::providers::ReviewProvider> = Arc::new(
        nitpik::providers::rig::RigProvider::new(config.provider.clone(), repo.to_path_buf())
            .expect("construct provider"),
    );

    // Measure the *shipped default engine*: run `auto`, which resolves the
    // always-on lenses (security, correctness) plus any conditional lenses the
    // diff-substance triage selects — not the fixture's declared profiles.
    // `profiles` is retained for documentation/back-compat of the fixtures.
    let _ = profiles;
    let options = nitpik::review::ReviewOptions {
        profiles: vec!["auto".to_string()],
        verify: true,
        no_cache: true,
        ..Default::default()
    };
    let agent_defs =
        nitpik::review::resolve_agents(Some(provider.as_ref()), &options, config, &diffs, repo)
            .await
            .expect("resolve agents")
            .agents;
    let progress = Arc::new(nitpik::progress::ProgressTracker::new(&[], &[], false));

    // A total provider failure (all tasks erroring) now surfaces as an
    // Err from the engine, so `.expect` fails the run loudly instead of
    // scoring a false "0 findings". Partial failures still return — warn
    // so they don't silently skew the scorecard.
    let output = nitpik::review::execute_review(
        provider,
        config,
        &repo.to_string_lossy(),
        &diffs,
        false,
        &agent_defs,
        baseline,
        &options,
        progress,
    )
    .await
    .expect("review");

    if output.result.failed_tasks > 0 {
        eprintln!(
            "  ⚠ {} review task(s) failed — scorecard for this case may be skewed",
            output.result.failed_tasks
        );
    }
    output.findings
}

#[tokio::test]
#[ignore = "makes real LLM calls; run with --ignored and an API key"]
async fn eval_corpus_scorecard() {
    if !has_api_key() {
        return;
    }
    let config = nitpik::config::Config::load(None, &nitpik::env::Env::real()).unwrap_or_default();

    // Collect case dirs (those with an expected.json), sorted for stable output.
    let mut case_dirs: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
        .expect("read eval fixtures dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("expected.json").exists())
        .collect();
    case_dirs.sort();
    assert!(!case_dirs.is_empty(), "no eval cases found");

    let mut cases = Vec::new();
    for dir in &case_dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let raw = std::fs::read_to_string(dir.join("expected.json")).expect("read expected.json");
        let case: ExpectedCase =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{name}/expected.json: {e}"));

        let (repo, _tmp) = setup_repo(dir).await;
        let findings = review_case(&repo, &case.profiles, &config).await;
        eprintln!("  · {name}: {} finding(s)", findings.len());
        // Dump each finding's anchor so a miss (a finding outside the
        // label's ±tolerance window) is diagnosable without a re-run.
        for f in &findings {
            eprintln!("      {}:{} [{}] {}", f.file, f.line, f.severity, f.title);
        }
        cases.push(score_case(&name, &case, &findings));
    }

    let card = Scorecard { cases };
    eprintln!("{}", card.render());

    if let Ok(path) = std::env::var("NITPIK_EVAL_BASELINE") {
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&card.to_json()).unwrap(),
        )
        .expect("write baseline");
        eprintln!("  wrote baseline → {path}");
    }
}
