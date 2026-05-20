//! Live smoke test for the offline NMT provider. Requires building
//! with `--features nmt` and passing a CrispASR-compatible GGUF
//! model path:
//!
//! ```bash
//! cargo run -p crisp-docx-llm --features nmt --example nmt_smoke -- \
//!     /path/to/m2m100-418m-q8_0.gguf
//! ```
//!
//! Verifies a couple of English↔German pairs work entirely offline.

use crisp_docx_llm::{LlmTranslator, ProviderConfig, ProviderKind};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let model_path = std::env::args()
        .nth(1)
        .expect("usage: nmt_smoke <gguf-path>");

    let translator = LlmTranslator::new()
        .add_provider(ProviderConfig {
            kind: ProviderKind::Nmt,
            api_key: None,
            model: model_path,
            base_url: None,
        })
        .unwrap();

    let pairs = [
        ("The dog is sleeping.", "English", "German"),
        ("Hello, world.", "English", "German"),
        ("Ich liebe Hunde.", "German", "English"),
    ];

    for (text, src, tgt) in pairs.iter() {
        match translator.translate_text(text, src, tgt).await {
            Ok(out) => println!("{src} → {tgt}\n  IN:  {text:?}\n  OUT: {out:?}\n"),
            Err(e) => println!("{src} → {tgt}: ERROR: {e}\n"),
        }
    }
}
