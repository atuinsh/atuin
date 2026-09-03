//! Deterministic synthetic shell-history generator.
//!
//! Produces command lines with a realistic shape: a heavily-duplicated head of
//! common commands, a body of templated commands with varying arguments, and a
//! small long tail of pipelines, one-liners, and rare very long commands.
//! Shaped after a real 168k-line history (~30% unique commands, short
//! commands dominating, mean ~36 bytes — the synthetic mean lands somewhat
//! below that, which affects absolute per-line cost but not engine
//! comparisons). Same seed → same corpus, so results are comparable across
//! runs and machines.

use easy_cast::Conv;

#[must_use]
pub fn generate(n: usize, seed: u64) -> Vec<String> {
    let mut rng = Rng::new(seed);
    (0..n).map(|_| gen_one(&mut rng)).collect()
}

// xorshift64* — no external deps, deterministic across platforms
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::conv(self.next() % u64::conv(n))
    }

    fn pick(&mut self, xs: &[&'static str]) -> &'static str {
        xs[self.below(xs.len())]
    }

    // min of two uniform draws biases toward the front of the list,
    // approximating the zipf-like frequency of real command usage
    fn pick_common(&mut self, xs: &[&'static str]) -> &'static str {
        let i = self.below(xs.len()).min(self.below(xs.len()));
        xs[i]
    }

    // high-cardinality argument, so templated commands mostly stay unique
    // (matching the ~30% unique commands of real history after the
    // heavily-repeated head)
    fn arg(&mut self) -> u64 {
        self.next() % 100_000
    }
}

const COMMON: &[&str] = &[
    "ls",
    "git status",
    "ls -la",
    "git diff",
    "cd ..",
    "git pull",
    "git push",
    "cargo build",
    "cargo test",
    "make",
    "pwd",
    "clear",
    "git log",
    "npm install",
    "npm test",
    "docker ps",
    "cargo check",
    "git stash",
    "git stash pop",
    "docker compose up",
    "docker compose up -d",
    "docker compose down",
    "cargo run",
    "yarn dev",
    "kubectl get pods",
    "git checkout main",
    "git rebase main",
    "brew update",
    "htop",
    "vim .",
    "code .",
    "make test",
    "python3 -m http.server",
    "nvim",
    "history",
    "df -h",
    "du -sh .",
    "uptime",
    "exit",
];

const WORDS: &[&str] = &[
    "server", "client", "config", "search", "index", "daemon", "history", "sync", "auth", "user",
    "session", "record", "import", "export", "test", "bench", "release", "deploy", "backup",
    "cache", "worker", "queue", "metrics", "logs",
];

const DIRS: &[&str] = &[
    "~/src/project",
    "./crates",
    "/tmp/scratch",
    "~/dotfiles",
    "./scripts",
    "../backend",
    "./src",
    "~/notes",
    "/var/log",
    "./ui",
];

const FILES: &[&str] = &[
    "main.rs",
    "lib.rs",
    "config.toml",
    "README.md",
    "Cargo.toml",
    "notes.md",
    "test.py",
    "deploy.sh",
    "data.json",
    "schema.sql",
];

const BRANCHES: &[&str] =
    &["main", "develop", "feature/search", "fix/sync-timeout", "chore/deps", "release-18"];

const HOSTS: &[&str] =
    &["prod-web-01", "staging.internal", "db.example.com", "api.example.com", "10.0.0.42"];

const USERS: &[&str] = &["root", "deploy", "ellie", "admin"];

const IMAGES: &[&str] = &["ubuntu:24.04", "postgres:16", "redis:7", "alpine", "rust:1.97"];

fn gen_one(rng: &mut Rng) -> String {
    match rng.below(100) {
        // exact repeats of common commands — the deduplicatable head
        0..=37 => rng.pick_common(COMMON).to_string(),
        38..=57 => match rng.below(5) {
            0 => format!("git commit -m '{} {} {}'", rng.pick(WORDS), rng.pick(WORDS), rng.arg()),
            1 => format!("git checkout {}", rng.pick(BRANCHES)),
            2 => format!("git push origin {}", rng.pick(BRANCHES)),
            3 => format!("git rebase -i HEAD~{}", 1 + rng.below(9)),
            _ => format!("git log --oneline -{}", 5 + rng.below(20)),
        },
        58..=71 => match rng.below(5) {
            0 => format!("vim {}/{}-{}.rs", rng.pick(DIRS), rng.pick(WORDS), rng.arg()),
            1 => format!("cat {}", rng.pick(FILES)),
            2 => format!("cp {} {}/", rng.pick(FILES), rng.pick(DIRS)),
            3 => format!("rm -rf {}", rng.pick(DIRS)),
            _ => format!("tar -xzf {}-{}.tar.gz", rng.pick(WORDS), rng.arg()),
        },
        72..=81 => match rng.below(4) {
            0 => format!("cargo run --release -p {}", rng.pick(WORDS)),
            1 => format!("npm run {}", rng.pick(WORDS)),
            2 => format!("make {}", rng.pick(WORDS)),
            _ => format!("python3 {}.py --{} {}", rng.pick(WORDS), rng.pick(WORDS), rng.arg()),
        },
        82..=89 => match rng.below(4) {
            0 => format!("ssh {}@{}", rng.pick(USERS), rng.pick(HOSTS)),
            1 => format!("docker run -it --rm {} bash", rng.pick(IMAGES)),
            2 => format!("kubectl get pods -n {}", rng.pick(WORDS)),
            _ => format!(
                "curl -s https://{}/api/{}/{} | jq .{}",
                rng.pick(HOSTS),
                rng.pick(WORDS),
                rng.arg(),
                rng.pick(WORDS)
            ),
        },
        90..=94 => match rng.below(3) {
            0 => format!(
                "grep -rn '{}-{}' {} | head -{}",
                rng.pick(WORDS),
                rng.arg(),
                rng.pick(DIRS),
                5 + rng.below(20)
            ),
            1 => format!("history | grep {}", rng.pick(WORDS)),
            _ => format!("ps aux | grep {} | awk '{{print $2}}' | xargs kill -9", rng.pick(WORDS)),
        },
        95..=96 => format!(
            "for f in {}/*.{}; do {} \"$f\" >> {}.log; done && tail -n {} {}.log | sort | uniq -c \
             | sort -rn",
            rng.pick(DIRS),
            rng.pick(&["rs", "log", "json", "txt"]),
            rng.pick(&["cat", "wc -l", "sha256sum"]),
            rng.pick(WORDS),
            10 + rng.below(90),
            rng.pick(WORDS),
        ),
        // very long command lines — rare but present in real histories (the
        // real corpus this is calibrated against tops out near 12KB)
        97 => {
            let mut cmd = format!("docker run --name {} ", rng.pick(WORDS));
            for _ in 0..6 + rng.below(10) {
                cmd.push_str(&format!(
                    "-e {}_{}={} ",
                    rng.pick(WORDS).to_uppercase(),
                    rng.pick(WORDS).to_uppercase(),
                    rng.next() % 100_000,
                ));
            }
            cmd.push_str(&format!(
                "-v {}:{} {} {} --{} {}",
                rng.pick(DIRS),
                rng.pick(DIRS),
                rng.pick(IMAGES),
                rng.pick(WORDS),
                rng.pick(WORDS),
                rng.pick(WORDS),
            ));
            cmd
        }
        // small unicode tail — real histories are mostly-but-not-all ASCII
        _ => match rng.below(3) {
            0 => format!("echo 'déjà vu {}'", rng.pick(WORDS)),
            1 => format!("vim メモ-{}.md", rng.pick(WORDS)),
            _ => format!("git commit -m 'améliorer {}'", rng.pick(WORDS)),
        },
    }
}
