//! Page metadata: title, byline, published, site, main image, base URL.

use scraper::Html;

pub struct Meta {
    pub title: Option<String>,
    pub byline: Option<String>,
    pub published: Option<String>,
    pub site: Option<String>,
    /// The page's main image: one absolute https URL, from og:image,
    /// twitter:image, JSON-LD Product.image or link rel=image_src
    /// (in that order).
    pub image: Option<String>,
    /// Meta description or og:description : a one-line summary
    /// agents can use to decide relevance without reading.
    pub description: Option<String>,
    /// Canonical URL from <link rel="canonical"> : the
    /// authoritative version of the page (dedup signal).
    #[allow(dead_code)]
    pub canonical: Option<String>,
}

pub fn base_url(doc: &Html) -> Option<String> {
    let sel = scraper::Selector::parse("base[href]").ok()?;
    doc.select(&sel)
        .next()
        .and_then(|b| b.value().attr("href"))
        .map(|s| s.to_string())
}

/// `base` is the page (or `<base href>`) URL that relative image
/// references resolve against.
pub fn metadata(doc: &Html, base: &str) -> Meta {
    let title = meta_attr(doc, "meta[property='og:title']", "content")
        .or_else(|| meta_attr(doc, "meta[name='twitter:title']", "content"))
        .or_else(|| {
            let sel = scraper::Selector::parse("title").ok()?;
            doc.select(&sel)
                .next()
                .map(|t| t.text().collect::<String>().trim().to_string())
        })
        .filter(|t| !t.is_empty());

    let byline = meta_attr(doc, "meta[name='author']", "content")
        .or_else(|| meta_attr(doc, "meta[property='article:author']", "content"))
        .or_else(|| json_ld_find(doc, "\"author\"", "\"name\""));

    let published = meta_attr(doc, "meta[property='article:published_time']", "content")
        .or_else(|| meta_attr(doc, "meta[name='date']", "content"))
        .or_else(|| meta_attr(doc, "meta[itemprop='datePublished']", "content"))
        .or_else(|| {
            let sel = scraper::Selector::parse("time[datetime]").ok()?;
            doc.select(&sel)
                .next()
                .and_then(|t| t.value().attr("datetime"))
                .map(|s| s.to_string())
        })
        .or_else(|| json_ld_find(doc, "\"datePublished\"", ""))
        .map(|d| d.chars().take(10).collect()); // date part only

    let site = meta_attr(doc, "meta[property='og:site_name']", "content")
        .or_else(|| meta_attr(doc, "meta[name='application-name']", "content"));

    let image = main_image(doc, base);

    let description = meta_attr(doc, "meta[name='description']", "content")
        .or_else(|| meta_attr(doc, "meta[property='og:description']", "content"))
        .or_else(|| meta_attr(doc, "meta[name='twitter:description']", "content"))
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());

    let canonical = scraper::Selector::parse("link[rel='canonical']")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .and_then(|l| l.value().attr("href"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Meta {
        title,
        byline,
        published,
        site,
        image,
        description,
        canonical,
    }
}

/// Longest image URL kept; anything longer is dropped, not truncated.
const MAX_IMAGE_URL_LEN: usize = 1024;

/// The page's main image as one absolute https URL. Sources in order:
/// og:image (and its secure_url/url variants), twitter:image, JSON-LD
/// Product image, link rel=image_src. A source whose value fails
/// validation is skipped in favour of the next candidate.
fn main_image(doc: &Html, base: &str) -> Option<String> {
    let base = url::Url::parse(base).ok();
    let ok = |raw: &str| clean_image_url(raw, base.as_ref());
    const META: [&str; 5] = [
        "meta[property='og:image']",
        "meta[property='og:image:secure_url']",
        "meta[property='og:image:url']",
        "meta[name='twitter:image']",
        "meta[name='twitter:image:src']",
    ];
    // og:* first, then twitter:*; within a group, every tag counts
    // (a page may repeat og:image) in document order.
    for group in [&META[..3], &META[3..]] {
        for sel in group {
            let Ok(sel) = scraper::Selector::parse(sel) else {
                continue;
            };
            for m in doc.select(&sel) {
                if let Some(u) = m.value().attr("content").and_then(ok) {
                    return Some(u);
                }
            }
        }
    }
    if let Some(u) = json_ld_images(doc).iter().find_map(|c| ok(c)) {
        return Some(u);
    }
    let sel = scraper::Selector::parse("link[rel~='image_src']").ok()?;
    doc.select(&sel)
        .find_map(|l| l.value().attr("href").and_then(ok))
}

/// Candidate image strings from every JSON-LD block: Product and
/// ProductGroup items (including inside `@graph` or top-level arrays)
/// and any other item carrying `offers`.
fn json_ld_images(doc: &Html) -> Vec<String> {
    let Ok(sel) = scraper::Selector::parse("script[type='application/ld+json']") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for script in doc.select(&sel) {
        let text: String = script.text().collect();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) {
            collect_product_images(&v, 0, &mut out);
        }
    }
    out
}

fn collect_product_images(v: &serde_json::Value, depth: usize, out: &mut Vec<String>) {
    use serde_json::Value;
    if depth > 6 {
        return;
    }
    match v {
        Value::Array(items) => {
            for i in items {
                collect_product_images(i, depth + 1, out);
            }
        }
        Value::Object(map) => {
            if let Some(g) = map.get("@graph") {
                collect_product_images(g, depth + 1, out);
            }
            let is_product = match map.get("@type") {
                Some(Value::String(t)) => is_product_type(t),
                Some(Value::Array(ts)) => {
                    ts.iter().any(|t| t.as_str().is_some_and(is_product_type))
                }
                _ => false,
            };
            if (is_product || map.contains_key("offers"))
                && let Some(img) = map.get("image")
                && let Some(s) = first_image_string(img, 0)
            {
                out.push(s);
            }
        }
        _ => {}
    }
}

fn is_product_type(t: &str) -> bool {
    let t = t.rsplit(['/', '#']).next().unwrap_or(t);
    t == "Product" || t == "ProductGroup"
}

/// `image` may be a string, an array (first usable entry wins) or an
/// ImageObject with `url` / `contentUrl`.
fn first_image_string(v: &serde_json::Value, depth: usize) -> Option<String> {
    use serde_json::Value;
    if depth > 3 {
        return None;
    }
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Array(items) => items.iter().find_map(|i| first_image_string(i, depth + 1)),
        Value::Object(m) => ["url", "contentUrl"]
            .iter()
            .find_map(|k| m.get(*k).and_then(|u| first_image_string(u, depth + 1))),
        _ => None,
    }
}

/// Resolve `raw` against `base` and keep it only if it is a plain
/// https URL: no credentials, no whitespace or control characters,
/// at most [`MAX_IMAGE_URL_LEN`] chars. HTML entities are decoded
/// (JSON-LD strings are not entity-decoded by the HTML parser).
fn clean_image_url(raw: &str, base: Option<&url::Url>) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > MAX_IMAGE_URL_LEN * 2 {
        return None;
    }
    let decoded = decode_entities(raw);
    if decoded.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let resolved = match base {
        Some(b) => b.join(&decoded).ok()?,
        None => url::Url::parse(&decoded).ok()?,
    };
    if resolved.scheme() != "https"
        || !resolved.username().is_empty()
        || resolved.password().is_some()
        || resolved.host_str().is_none_or(str::is_empty)
    {
        return None;
    }
    let s: String = resolved.into();
    (s.len() <= MAX_IMAGE_URL_LEN).then_some(s)
}

/// Decode HTML character references via the HTML parser itself.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    Html::parse_fragment(s)
        .root_element()
        .text()
        .collect::<String>()
}

fn meta_attr(doc: &Html, selector: &str, attr: &str) -> Option<String> {
    let sel = scraper::Selector::parse(selector).ok()?;
    doc.select(&sel)
        .next()
        .and_then(|m| m.value().attr(attr))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Minimal hand-rolled JSON-LD value finder. JSON-LD in the
/// wild is malformed often enough that a real parser fails
/// more than a tolerant string search.
fn json_ld_find(doc: &Html, key: &str, subkey: &str) -> Option<String> {
    let sel = scraper::Selector::parse("script[type='application/ld+json']").ok()?;
    for script in doc.select(&sel) {
        let text: String = script.text().collect();
        let Some(ki) = text.find(key) else { continue };
        // The window end is a byte offset into text that is usually
        // raw UTF-8 (only Wikipedia-style output \u-escapes it), so
        // it must be pulled back onto a char boundary or the slice
        // panics on an ordinary non-English page.
        let start = ki + key.len();
        let end = text.floor_char_boundary(text.len().min(start + 300));
        let region = &text[start..end];
        let hay = if subkey.is_empty() {
            region
        } else {
            let Some(si) = region.find(subkey) else {
                continue;
            };
            &region[si + subkey.len()..]
        };
        // First "..." string after the key.
        let Some(q1) = hay.find('"') else { continue };
        let Some(q2) = hay[q1 + 1..].find('"') else {
            continue;
        };
        let val = &hay[q1 + 1..q1 + 1 + q2];
        if !val.is_empty() && val.len() < 200 {
            return Some(decode_unicode_escapes(val));
        }
    }
    None
}

/// Decode JSON \uXXXX escape sequences to actual characters.
/// Wikipedia and other sites encode non-ASCII author names
/// as Unicode escapes in JSON-LD. Without this, the byline
/// shows \u7ef4\u57fa instead of 维基.
fn decode_unicode_escapes(s: &str) -> String {
    if !s.contains("\\u") {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find("\\u") {
        result.push_str(&rest[..pos]);
        rest = &rest[pos + 2..];
        // `get` (not a slice) : the 4 bytes after \u are only a
        // valid str if they're ASCII hex, and a multibyte char in
        // there would otherwise panic the slice.
        if let Some(hex) = rest.get(..4)
            && let Ok(code) = u32::from_str_radix(hex, 16)
            && let Some(ch) = char::from_u32(code)
        {
            result.push(ch);
            rest = &rest[4..];
            continue;
        }
        // Invalid escape : keep literal.
        result.push_str("\\u");
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // json_ld_find used to slice the ld+json text at a fixed +300
    // BYTE offset after the key. Most CMSs emit raw UTF-8 in ld+json
    // (only Wikipedia-style output \u-escapes it), so on an ordinary
    // non-English page that offset lands inside a multibyte char
    // about half the time -- a str-slice panic, which under this
    // crate's panic = "abort" release profile takes the whole MCP
    // daemon down on a plain fetch.
    #[test]
    fn json_ld_window_end_inside_multibyte_char_does_not_panic() {
        // One ASCII byte then 3-byte chars: byte 300 after the key is
        // never a char boundary (boundaries sit at 1 + 3k).
        let pad: String = std::iter::once('x')
            .chain(std::iter::repeat_n('年', 150))
            .collect();
        let html = format!(
            r#"<html><head><script type="application/ld+json">{{"author"{pad},"name":"X"}}</script></head><body></body></html>"#
        );
        let doc = Html::parse_document(&html);
        // Must not panic; the value is whatever the tolerant search
        // finds (or nothing) -- the assertion is that we get here.
        let _ = json_ld_find(&doc, "\"author\"", "\"name\"");
        let _ = json_ld_find(&doc, "\"datePublished\"", "");
    }

    // decode_unicode_escapes sliced `rest[..4]` after every `\u`,
    // which panics when a multibyte char sits inside those 4 bytes.
    #[test]
    fn unicode_escape_decoder_survives_multibyte_after_backslash_u() {
        // rest = "aëüller": a(1) ë(2) ü(2) -> byte 4 is inside 'ü'.
        assert_eq!(decode_unicode_escapes("Zo\\uaëüller"), "Zo\\uaëüller");
        // Still decodes real escapes.
        assert_eq!(decode_unicode_escapes("\\u7ef4\\u57fa"), "维基");
    }

    const PAGE: &str = "https://shop.example.com/products/mug";

    fn image_of(head: &str) -> Option<String> {
        let html = format!("<html><head>{head}</head><body><p>x</p></body></html>");
        metadata(&Html::parse_document(&html), PAGE).image
    }

    fn ld(json: &str) -> String {
        format!(r#"<script type="application/ld+json">{json}</script>"#)
    }

    #[test]
    fn image_from_og_image() {
        let got = image_of(r#"<meta property="og:image" content="https://cdn.example.com/a.jpg">"#);
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/a.jpg"));
    }

    #[test]
    fn image_from_og_image_secure_url_and_url() {
        let got = image_of(
            r#"<meta property="og:image:secure_url" content="https://cdn.example.com/s.jpg">"#,
        );
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/s.jpg"));
        let got =
            image_of(r#"<meta property="og:image:url" content="https://cdn.example.com/u.jpg">"#);
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/u.jpg"));
    }

    #[test]
    fn image_from_twitter_image_and_src() {
        let got =
            image_of(r#"<meta name="twitter:image" content="https://cdn.example.com/t.jpg">"#);
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/t.jpg"));
        let got =
            image_of(r#"<meta name="twitter:image:src" content="https://cdn.example.com/ts.jpg">"#);
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/ts.jpg"));
    }

    #[test]
    fn image_from_json_ld_product_string_array_and_object() {
        let got = image_of(&ld(
            r#"{"@type":"Product","image":"https://cdn.example.com/p.jpg"}"#,
        ));
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/p.jpg"));
        let got = image_of(&ld(
            r#"{"@type":"Product","image":["https://cdn.example.com/1.jpg","https://cdn.example.com/2.jpg"]}"#,
        ));
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/1.jpg"));
        let got = image_of(&ld(
            r#"{"@type":"Product","image":{"@type":"ImageObject","url":"https://cdn.example.com/o.jpg"}}"#,
        ));
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/o.jpg"));
        let got = image_of(&ld(
            r#"{"@type":"Product","image":[{"@type":"ImageObject","contentUrl":"https://cdn.example.com/c.jpg"}]}"#,
        ));
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/c.jpg"));
    }

    #[test]
    fn image_from_json_ld_graph_group_and_offers() {
        let got = image_of(&ld(
            r#"{"@graph":[{"@type":"WebSite"},{"@type":["Thing","Product"],"image":"https://cdn.example.com/g.jpg"}]}"#,
        ));
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/g.jpg"));
        let got = image_of(&ld(
            r#"{"@type":"ProductGroup","image":"https://cdn.example.com/pg.jpg"}"#,
        ));
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/pg.jpg"));
        let got = image_of(&ld(
            r#"{"@type":"IndividualProduct","offers":{"@type":"Offer"},"image":"https://cdn.example.com/of.jpg"}"#,
        ));
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/of.jpg"));
    }

    #[test]
    fn image_json_ld_ignores_non_product_images() {
        let got = image_of(&ld(
            r#"{"@type":"Organization","image":"https://cdn.example.com/logo.jpg"}"#,
        ));
        assert_eq!(got, None);
    }

    #[test]
    fn image_from_link_image_src() {
        let got = image_of(r#"<link rel="image_src" href="https://cdn.example.com/l.jpg">"#);
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/l.jpg"));
    }

    #[test]
    fn image_source_precedence() {
        let all = format!(
            r#"{}{}{}<meta name="twitter:image" content="https://cdn.example.com/tw.jpg"><meta property="og:image" content="https://cdn.example.com/og.jpg">"#,
            ld(r#"{"@type":"Product","image":"https://cdn.example.com/ld.jpg"}"#),
            r#"<link rel="image_src" href="https://cdn.example.com/link.jpg">"#,
            "",
        );
        assert_eq!(
            image_of(&all).as_deref(),
            Some("https://cdn.example.com/og.jpg")
        );
        let no_og = format!(
            r#"{}<link rel="image_src" href="https://cdn.example.com/link.jpg"><meta name="twitter:image" content="https://cdn.example.com/tw.jpg">"#,
            ld(r#"{"@type":"Product","image":"https://cdn.example.com/ld.jpg"}"#),
        );
        assert_eq!(
            image_of(&no_og).as_deref(),
            Some("https://cdn.example.com/tw.jpg")
        );
        let ld_link = format!(
            r#"{}<link rel="image_src" href="https://cdn.example.com/link.jpg">"#,
            ld(r#"{"@type":"Product","image":"https://cdn.example.com/ld.jpg"}"#),
        );
        assert_eq!(
            image_of(&ld_link).as_deref(),
            Some("https://cdn.example.com/ld.jpg")
        );
    }

    #[test]
    fn image_invalid_higher_source_falls_through() {
        let got = image_of(
            r#"<meta property="og:image" content="http://cdn.example.com/insecure.jpg"><meta name="twitter:image" content="https://cdn.example.com/ok.jpg">"#,
        );
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/ok.jpg"));
    }

    #[test]
    fn image_resolves_relative_and_protocol_relative() {
        let got = image_of(r#"<meta property="og:image" content="/img/a.jpg">"#);
        assert_eq!(got.as_deref(), Some("https://shop.example.com/img/a.jpg"));
        let got = image_of(r#"<meta property="og:image" content="b.jpg">"#);
        assert_eq!(
            got.as_deref(),
            Some("https://shop.example.com/products/b.jpg")
        );
        let got = image_of(r#"<meta property="og:image" content="//cdn.example.com/c.jpg">"#);
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/c.jpg"));
    }

    #[test]
    fn image_resolves_against_base_href() {
        let html =
            r#"<html><head><meta property="og:image" content="a.jpg"></head><body></body></html>"#;
        let m = metadata(&Html::parse_document(html), "https://static.example.com/x/");
        assert_eq!(
            m.image.as_deref(),
            Some("https://static.example.com/x/a.jpg")
        );
    }

    #[test]
    fn image_decodes_html_entities() {
        let got = image_of(
            r#"<meta property="og:image" content="https://cdn.example.com/a.jpg?w=1&amp;h=2">"#,
        );
        assert_eq!(
            got.as_deref(),
            Some("https://cdn.example.com/a.jpg?w=1&h=2")
        );
        let got = image_of(&ld(
            r#"{"@type":"Product","image":"https://cdn.example.com/a.jpg?w=1&amp;h=2"}"#,
        ));
        assert_eq!(
            got.as_deref(),
            Some("https://cdn.example.com/a.jpg?w=1&h=2")
        );
    }

    #[test]
    fn image_rejects_unsafe_or_malformed_urls() {
        let long = format!("https://cdn.example.com/{}.jpg", "a".repeat(1100));
        let cases = [
            "http://cdn.example.com/a.jpg".to_string(),
            "data:image/png;base64,AAAA".to_string(),
            "blob:https://cdn.example.com/uuid".to_string(),
            "javascript:alert(1)".to_string(),
            "https://user:pw@cdn.example.com/a.jpg".to_string(),
            "https://user@cdn.example.com/a.jpg".to_string(),
            "https://cdn.example.com/a b.jpg".to_string(),
            "https://cdn.example.com/a&#10;b.jpg".to_string(),
            "https://cdn.example.com/a\tb.jpg".to_string(),
            "ftp://cdn.example.com/a.jpg".to_string(),
            long,
            String::new(),
        ];
        for c in cases {
            let head = format!(r#"<meta property="og:image" content="{c}">"#);
            assert_eq!(image_of(&head), None, "should reject {c:?}");
        }
    }

    #[test]
    fn image_missing_is_none() {
        assert_eq!(image_of(""), None);
    }

    #[test]
    fn image_never_emits_more_than_one_url() {
        let got = image_of(
            r#"<meta property="og:image" content="https://cdn.example.com/1.jpg"><meta property="og:image" content="https://cdn.example.com/2.jpg">"#,
        );
        assert_eq!(got.as_deref(), Some("https://cdn.example.com/1.jpg"));
    }
}
