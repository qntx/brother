# Brother

Browser automation for AI agents, built on Chrome DevTools Protocol.

**Zero Node.js dependency** — pure Rust, directly controlling Chrome/Chromium via CDP.

## Features

- **Accessibility Snapshot + Refs** — capture the a11y tree with stable element refs (`e1`, `e2`, ...) for AI-friendly page observation
- **Ref-based interaction** — click, fill, type, hover, focus elements by ref without CSS selectors
- **CSS selector fallback** — traditional selector-based interaction when needed
- **Screenshot** — PNG/JPEG viewport capture
- **JavaScript evaluation** — run arbitrary JS and deserialize results
- **Navigation** — goto, back, forward, reload, wait for navigation
- **CLI** — `brother open`, `brother snapshot`, `brother click`, `brother screenshot`, `brother eval`, `brother text`

## Quick Start

### As a Library

```rust
use brother::{Browser, BrowserConfig};
use futures::StreamExt;

#[tokio::main]
async fn main() -> brother::Result<()> {
    let (browser, mut handler) = Browser::launch(BrowserConfig::default()).await?;
    tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser.new_page("https://example.com").await?;

    // AI-friendly: get accessibility snapshot with refs
    let snapshot = page.snapshot().await?;
    println!("{}", snapshot.tree());
    // - heading "Example Domain" [ref=e1] [level=1]
    // - link "More information..." [ref=e2]

    // Interact by ref
    page.click_ref("e2").await?;

    Ok(())
}
```

### As a CLI

```bash
# Navigate and print page info
brother open https://example.com

# Get accessibility snapshot
brother snapshot https://example.com
brother snapshot https://example.com --interactive --compact

# Screenshot
brother screenshot https://example.com -o page.png

# Evaluate JavaScript
brother eval https://example.com "document.title"

# Get text content
brother text https://example.com
brother text https://example.com -s "h1"

# JSON output for all commands
brother --json snapshot https://example.com
```

## Requirements

- Rust 1.85+
- Chrome or Chromium installed

## License

Licensed under the [Functional Source License, Version 1.1, Apache-2.0 Future License](LICENSE.md) (FSL-1.1-ALv2).

- You can use, modify, and redistribute for any purpose **except** competing use.
- Each version automatically converts to the [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0) two years after release.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project shall be licensed as above, without any additional terms or conditions.

---

<div align="center">

A **[QNTX](https://qntx.fun)** open-source project.

<a href="https://qntx.fun"><img alt="QNTX" width="369" src="https://raw.githubusercontent.com/qntx/.github/main/profile/qntx-banner.svg" /></a>

<!--prettier-ignore-->
Code is law. We write both.

</div>
