use akku::html::{escape, Html};

#[test]
fn html_public_api_and_escaping() {
    let raw = Html::from("<p>Hello</p>");
    assert_eq!(raw.as_str(), "<p>Hello</p>");
    assert_eq!(raw.clone().into_string(), "<p>Hello</p>");
    assert_eq!(
        escape("<script>alert(\"x\" & 'y')</script>"),
        "&lt;script&gt;alert(&#34;x&#34; &amp; &#39;y&#39;)&lt;/script&gt;"
    );
}
