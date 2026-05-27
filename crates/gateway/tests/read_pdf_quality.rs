//! Manual quality probe for the M4.1.1 `read_pdf` tool. Hits the live
//! EastMoney `pdf.dfcfw.com` mirror for a known 茅台 research report and
//! samples the extracted text so a human can eyeball the CJK fidelity.
//!
//! Marked `#[ignore]` (and behind `cargo test -- --ignored`) so CI runs
//! never reach out to the public network.

#[tokio::test]
#[ignore]
async fn extract_real_eastmoney_pdf_samples_cjk_text() {
    use std::io::Write;

    let url = "https://pdf.dfcfw.com/pdf/H3_AP202605251822844635_1.pdf";
    let client = reqwest::Client::new();
    let bytes = client
        .get(url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    println!("downloaded {} bytes", bytes.len());

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(&bytes).unwrap();
    let path = tmp.path().to_path_buf();
    let text = tokio::task::spawn_blocking(move || pdf_extract::extract_text(&path))
        .await
        .unwrap()
        .unwrap();
    let total = text.chars().count();
    let non_ascii = text.chars().filter(|c| !c.is_ascii()).count();
    let pct = 100.0 * non_ascii as f64 / total as f64;
    println!("total chars: {total}, non-ASCII: {non_ascii} ({pct:.1}%)");
    println!("--- first 600 chars ---");
    println!("{}", text.chars().take(600).collect::<String>());
    println!("--- chars 1000..1500 ---");
    println!("{}", text.chars().skip(1000).take(500).collect::<String>());
    // No hard semantic assertion — this is a hand-inspected probe — but
    // we do sanity-check that the extraction produced a non-trivial
    // amount of text. Hand-verified 2026-05-27: 茅台 5/25 research PDF
    // returns 8855 chars, 48.5% Chinese, publication-quality output.
    assert!(total > 1000, "extraction returned suspiciously little text");
    assert!(pct > 30.0, "expected ≥ 30% non-ASCII for a Chinese PDF");
}
