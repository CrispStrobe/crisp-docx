//! Live smoke test: align an English / German sentence pair with
//! paraphrase-multilingual-MiniLM-L12-v2.
//!
//! Run:
//!   cargo run -p crisp-docx-align --features crispembed \
//!     --example align_smoke -- <gguf-path>

use crisp_docx_align::{align_texts, Strategy};
use crispembed::CrispEmbed;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: align_smoke <multilingual-encoder.gguf>");

    let mut model = CrispEmbed::new(&path, 4).expect("load model");
    println!("dim:            {}", model.dim());
    println!("tokenizer_kind: {}", model.tokenizer_kind());

    let pairs = [
        ("The dog is sleeping.", "Der Hund schläft."),
        (
            "The classical tradition's final word on religious matters",
            "Das letzte Wort der klassischen Tradition zu religiösen Fragen",
        ),
    ];

    for (src, tgt) in pairs.iter() {
        for strategy in [Strategy::Intersection, Strategy::Itermax { min_sim: 0.4 }] {
            let a = align_texts(&mut model, src, tgt, strategy).unwrap();
            println!("\n--- {strategy:?} ---");
            println!("EN: {src:?}");
            println!("DE: {tgt:?}");
            println!(
                "source words ({}): {:?}",
                a.src_words.len(),
                a.src_words.iter().map(|w| &w.text).collect::<Vec<_>>()
            );
            println!(
                "target words ({}): {:?}",
                a.tgt_words.len(),
                a.tgt_words.iter().map(|w| &w.text).collect::<Vec<_>>()
            );
            println!("word edges:");
            for (sw, tw) in &a.word_edges {
                println!(
                    "  {sw:>2}: {:<20} ↔ {tw:>2}: {}",
                    a.src_words[*sw].text, a.tgt_words[*tw].text
                );
            }
        }
    }
}
