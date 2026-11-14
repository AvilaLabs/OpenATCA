//! Cross-check of sector altitudes against the FAA's PDF plots. The PDFs are never parsed here:
//! `pdftotext -raw` (poppler) prints each sector label as two lines, the sector id and then its
//! altitude in hundreds of feet (`B` / `26`, `GALO01` / `40`), and this reads those pairs. If
//! `pdftotext` is not installed the check reports [`PdfStatus::Unavailable`]: not verified.

use crate::model::{
    chart_count, facility_text, sector_name_text, sector_range_of_chart, Model, SectorIx,
};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of the PDF check of one chart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfStatus {
    Checked(PdfReport),
    /// Not verified, with the reason (no `pdftotext`, no PDF, unreadable).
    Unavailable(String),
}

/// Comparison of one chart with its PDF. Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfReport {
    pub chart: String,
    pub xml_sectors: usize,
    pub pdf_labels: usize,
    pub matched: usize,
    /// (sector id, altitude in the XML, altitude on the PDF)
    pub mismatched: Vec<(String, i32, i32)>,
    pub missing_in_pdf: Vec<String>,
    pub extra_in_pdf: Vec<String>,
}

/// A sector id as printed on a plot: capital letter first, then capitals and digits, at most 8.
fn is_id(l: &str) -> bool {
    let b = l.as_bytes();
    (1..=8).contains(&b.len())
        && b.first().is_some_and(u8::is_ascii_uppercase)
        && b.iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// The (sector id, altitude in feet) labels in `pdftotext -raw` text: an id token directly
/// followed by a token of 2 or 3 digits (hundreds of feet). Tokens are split on any white space,
/// because the plot sometimes puts a number and the next sector's id on one line.
pub fn pdf_labels_from_text(text: &str) -> Vec<(String, i32)> {
    let toks: Vec<&str> = text.split_whitespace().collect();
    let mut out = Vec::new();
    for w in toks.windows(2) {
        if let [id, num] = w {
            if is_id(id) && (2..=3).contains(&num.len()) && num.bytes().all(|c| c.is_ascii_digit())
            {
                if let Ok(h) = num.parse::<i32>() {
                    out.push(((*id).to_string(), h * 100));
                }
            }
        }
    }
    out
}

/// Shell: the text of a PDF by `pdftotext -raw`.
pub fn pdftotext(path: &Path) -> Result<String, String> {
    let out = Command::new("pdftotext")
        .arg("-raw")
        .arg(path)
        .arg("-")
        .output()
        .map_err(|e| format!("pdftotext not runnable: {e}"))?;
    if !out.status.success() {
        return Err(format!("pdftotext failed on {}", path.display()));
    }
    String::from_utf8(out.stdout).map_err(|_| "pdftotext output is not UTF-8".to_string())
}

/// The PDF file name of a chart in the FAA directory: `C90_MVA_FUS3.pdf`, `ZAU_TAV.pdf`.
pub fn pdf_path_for(dir: &Path, facility: &str, set: &str) -> PathBuf {
    if set == "TAV" {
        dir.join(format!("{facility}_TAV.pdf"))
    } else {
        dir.join(format!("{facility}_MVA_{set}.pdf"))
    }
}

/// Compares chart `chart` of `m` with the labels read from its PDF.
pub fn compare_chart(m: &Model, chart: usize, labels: &[(String, i32)]) -> PdfReport {
    let secs = sector_range_of_chart(m, chart);
    let fac = secs
        .clone()
        .next()
        .map(|i| facility_text(m, SectorIx(i as u32)))
        .unwrap_or_default();
    let set = m.chart_set.get(chart).map_or("?", |s| s.name());
    let mut rep = PdfReport {
        chart: format!("{fac} {set}"),
        xml_sectors: secs.len(),
        pdf_labels: labels.len(),
        matched: 0,
        mismatched: Vec::new(),
        missing_in_pdf: Vec::new(),
        extra_in_pdf: Vec::new(),
    };
    for i in secs.clone() {
        let id = sector_name_text(m, SectorIx(i as u32));
        let alt = m.min_alt_ft.get(i).copied().unwrap_or(0);
        let on_pdf: Vec<i32> = labels.iter().filter(|l| l.0 == id).map(|l| l.1).collect();
        if on_pdf.is_empty() {
            rep.missing_in_pdf.push(id);
        } else if on_pdf.contains(&alt) {
            rep.matched += 1;
        } else {
            rep.mismatched
                .push((id, alt, on_pdf.first().copied().unwrap_or(0)));
        }
    }
    for (id, _) in labels {
        let known = secs
            .clone()
            .any(|i| sector_name_text(m, SectorIx(i as u32)) == *id);
        if !known && !rep.extra_in_pdf.contains(id) {
            rep.extra_in_pdf.push(id.clone());
        }
    }
    rep
}

/// Runs the check for every chart of `m` whose PDF is in `dir`.
pub fn check_all(m: &Model, dir: &Path) -> Vec<PdfStatus> {
    (0..chart_count(m))
        .map(|c| {
            let secs = sector_range_of_chart(m, c);
            let fac = facility_text(m, SectorIx(secs.start as u32));
            let set = m.chart_set.get(c).map_or("?", |s| s.name());
            let path = pdf_path_for(dir, &fac, set);
            if !path.is_file() {
                return PdfStatus::Unavailable(format!("{} is absent", path.display()));
            }
            match pdftotext(&path) {
                Ok(t) => PdfStatus::Checked(compare_chart(m, c, &pdf_labels_from_text(&t))),
                Err(e) => PdfStatus::Unavailable(e),
            }
        })
        .collect()
}
