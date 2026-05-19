use crisp_docx_core::{open, save, strip_paragraph_bold};
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let src = args.next().expect("src");
    let dst = args.next().expect("dst");
    let mut pkg = open(&src)?;
    let n = strip_paragraph_bold(&mut pkg)?;
    save(&pkg, &dst)?;
    println!("unbolded {n} paragraphs -> {dst}");
    Ok(())
}
