//! Reading what uv prints when its output is piped, which is a line per event
//! rather than an animation. The version of uv is pinned, so these lines only
//! change with [`super::UV_VERSION`], and a line not recognised costs no more
//! than a bar that moves less.

use std::io::Write;

use crate::progress::Progress;

/// The bar's position, in thousandths of a step, at the stages of a step.
const RESOLVED: u64 = 100;
const PREPARED: u64 = 800;
const INSTALLED: u64 = 900;
const WHOLE: u64 = 1000;

/// What has been read of one run of uv.
#[derive(Debug, Default)]
pub struct UvOutput {
    packages: u64,
    downloading: u64,
    downloaded: u64,
    added: u64,
}

impl UvOutput {
    /// Whether uv has said it is done downloading, leaving the bar to this.
    pub fn is_finishing(&self) -> bool {
        self.downloaded > 0
    }

    /// Reports `line` to `progress`, if it says anything worth reporting.
    pub fn read<W: Write>(&mut self, line: &str, progress: &mut Progress<W>) {
        if let Some(rest) = line.strip_prefix("Resolved ") {
            self.packages = leading_number(rest);
            progress.share(RESOLVED, WHOLE);
        } else if let Some(rest) = line.strip_prefix("Downloading ") {
            self.downloading += 1;
            progress.note(&format!("downloading {}", rest.replace(" (download)", "")));
        } else if line.starts_with(" Downloaded ") {
            self.downloaded += 1;
            let span = PREPARED - RESOLVED;
            let done = RESOLVED + span * self.downloaded / self.downloading.max(1);
            progress.share(done.min(PREPARED), WHOLE);
        } else if line.starts_with("Prepared ") {
            progress.share(PREPARED, WHOLE);
        } else if line.starts_with("Installed ") {
            progress.share(INSTALLED, WHOLE);
        } else if let Some(rest) = line.trim_start().strip_prefix("Building ") {
            progress.note(&format!("building {}", package_name(rest)));
        } else if let Some(rest) = line.strip_prefix("warning: ") {
            progress.warn(rest);
        } else if let Some(rest) = line.strip_prefix(" + ") {
            self.added += 1;
            progress.note(&format!("+ {}", added(rest)));
            let span = WHOLE - INSTALLED;
            let done = INSTALLED + span * self.added / self.packages.max(self.added);
            progress.share(done, WHOLE);
        }
    }
}

fn leading_number(text: &str) -> u64 {
    text.split_whitespace()
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

/// `searxng @ file:///…` as `searxng`.
fn package_name(text: &str) -> &str {
    text.split(" @ ").next().unwrap_or(text)
}

/// `babel==2.18.0` as `babel 2.18.0`, without any `(from file:///…)` after it.
fn added(text: &str) -> String {
    let package = text.split(" (from ").next().unwrap_or(text);
    package.replacen("==", " ", 1)
}

#[cfg(test)]
mod tests {
    use super::UvOutput;
    use crate::progress::Progress;

    fn read(lines: &str) -> Progress<Vec<u8>> {
        let mut progress = Progress::plain(Vec::new());
        progress.plan(&[1]);
        progress.step("step");
        let mut output = UvOutput::default();
        for line in lines.lines() {
            output.read(line, &mut progress);
        }
        progress
    }

    #[test]
    fn a_package_install_reports_its_packages() {
        let progress = read(
            "Using Python 3.13.13 environment at: /tmp/venv
Resolved 2 packages in 365ms
Downloading babel (9.7MiB)
 Downloaded babel
Prepared 2 packages in 5.68s
Installed 2 packages in 6ms
 + annotated-doc==0.0.5
 + babel==2.18.0
",
        );
        assert_eq!(
            progress.written(),
            "step
      downloading babel (9.7MiB)
      + annotated-doc 0.0.5
      + babel 2.18.0
"
        );
    }

    #[test]
    fn a_python_install_names_what_it_added() {
        let progress = read(
            "Downloading cpython-3.13.13-linux-x86_64-gnu (download) (33.2MiB)
 Downloaded cpython-3.13.13-linux-x86_64-gnu (download)
Installed Python 3.13.13 in 9.64s
 + cpython-3.13.13-linux-x86_64-gnu
",
        );
        assert_eq!(
            progress.written(),
            "step
      downloading cpython-3.13.13-linux-x86_64-gnu (33.2MiB)
      + cpython-3.13.13-linux-x86_64-gnu
"
        );
    }

    #[test]
    fn a_local_build_drops_its_file_url() {
        let progress = read(
            "   Building searxng @ file:///C:/Users/x/source/searxng-094c
      Built searxng @ file:///C:/Users/x/source/searxng-094c
 + searxng==2026.8.14+094c33d40 (from file:///C:/Users/x/source/searxng-094c)
",
        );
        assert_eq!(
            progress.written(),
            "step
      building searxng
      + searxng 2026.8.14+094c33d40
"
        );
    }

    #[test]
    fn lines_it_does_not_know_are_passed_over() {
        let progress = read("something new\nno idea\n");
        assert_eq!(progress.written(), "step\n");
    }

    #[test]
    fn a_warning_from_uv_is_passed_on() {
        let progress = read("warning: something odd\n");
        assert_eq!(progress.written(), "step\nwarning: something odd\n");
    }
}
