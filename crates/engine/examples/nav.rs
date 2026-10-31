//! Prints a document's outline and the links on one page.
//!
//! usage: cargo run --release -p engine --example nav -- <pdf> [page, 1-based]

use engine::{OutlineItem, Target};

fn show(target: &Option<Target>) -> String {
    match target {
        Some(Target::Page { page, y: Some(y) }) => format!("page {} at {y:.0} pt", page + 1),
        Some(Target::Page { page, y: None }) => format!("page {}", page + 1),
        Some(Target::Uri(u)) => u.clone(),
        None => "nowhere".to_owned(),
    }
}

fn print(items: &[OutlineItem], depth: usize, shown: &mut usize) {
    for item in items {
        if *shown >= 40 {
            return;
        }
        *shown += 1;
        let mark = if item.children.is_empty() { ' ' } else if item.open { '-' } else { '+' };
        println!("{:indent$}{mark} {} -> {}", "", item.title, show(&item.target), indent = depth * 2);
        print(&item.children, depth + 1, shown);
    }
}

fn count(items: &[OutlineItem]) -> usize {
    items.iter().map(|i| 1 + count(&i.children)).sum()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: nav <pdf> [page]");
    let page = args.next().and_then(|p| p.parse::<usize>().ok()).unwrap_or(1).saturating_sub(1);
    let doc = engine::open(path.as_ref()).expect("opening");
    let outline = doc.outline();
    println!("outline: {} entries", count(&outline));
    print(&outline, 1, &mut 0);
    let links = doc.page_links(page);
    println!("page {}: {} links", page + 1, links.len());
    for l in links.iter().take(20) {
        println!("  [{:.0} {:.0} {:.0} {:.0}] -> {}", l.rect[0], l.rect[1], l.rect[2], l.rect[3], show(&Some(l.target.clone())));
    }
}
