(function() {
    var type_impls = Object.fromEntries([["smithay",[]],["winit",[]],["x11rb",[["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-Serialize-for-u8\" class=\"impl\"><a href=\"#impl-Serialize-for-u8\" class=\"anchor\">§</a><h3 class=\"code-header\">impl <a class=\"trait\" href=\"x11rb/x11_utils/trait.Serialize.html\" title=\"trait x11rb::x11_utils::Serialize\">Serialize</a> for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a></h3></section></summary><div class=\"impl-items\"><details class=\"toggle\" open><summary><section id=\"associatedtype.Bytes\" class=\"associatedtype trait-impl\"><a href=\"#associatedtype.Bytes\" class=\"anchor\">§</a><h4 class=\"code-header\">type <a href=\"x11rb/x11_utils/trait.Serialize.html#associatedtype.Bytes\" class=\"associatedtype\">Bytes</a> = [<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a>; <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.array.html\">1</a>]</h4></section></summary><div class='docblock'>The value returned by <code>serialize</code>. <a href=\"x11rb/x11_utils/trait.Serialize.html#associatedtype.Bytes\">Read more</a></div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.serialize\" class=\"method trait-impl\"><a href=\"#method.serialize\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"x11rb/x11_utils/trait.Serialize.html#tymethod.serialize\" class=\"fn\">serialize</a>(&amp;self) -&gt; &lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a> as <a class=\"trait\" href=\"x11rb/x11_utils/trait.Serialize.html\" title=\"trait x11rb::x11_utils::Serialize\">Serialize</a>&gt;::<a class=\"associatedtype\" href=\"x11rb/x11_utils/trait.Serialize.html#associatedtype.Bytes\" title=\"type x11rb::x11_utils::Serialize::Bytes\">Bytes</a></h4></section></summary><div class='docblock'>Serialize this value into X11 raw bytes.</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.serialize_into\" class=\"method trait-impl\"><a href=\"#method.serialize_into\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"x11rb/x11_utils/trait.Serialize.html#tymethod.serialize_into\" class=\"fn\">serialize_into</a>(&amp;self, bytes: &amp;mut <a class=\"struct\" href=\"https://doc.rust-lang.org/nightly/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a>&gt;)</h4></section></summary><div class='docblock'>Serialize this value into X11 raw bytes, appending the result into <code>bytes</code>. <a href=\"x11rb/x11_utils/trait.Serialize.html#tymethod.serialize_into\">Read more</a></div></details></div></details>","Serialize","x11rb::protocol::xproto::Keycode","x11rb::protocol::xproto::Button","x11rb::protocol::shape::Op","x11rb::protocol::shape::Kind","x11rb::protocol::xinput::KeyCode","x11rb::protocol::xinput::EventTypeBase","x11rb::protocol::xkb::String8"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-TryParse-for-u8\" class=\"impl\"><a href=\"#impl-TryParse-for-u8\" class=\"anchor\">§</a><h3 class=\"code-header\">impl <a class=\"trait\" href=\"x11rb/x11_utils/trait.TryParse.html\" title=\"trait x11rb::x11_utils::TryParse\">TryParse</a> for <a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a></h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.try_parse\" class=\"method trait-impl\"><a href=\"#method.try_parse\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"x11rb/x11_utils/trait.TryParse.html#tymethod.try_parse\" class=\"fn\">try_parse</a>(value: &amp;[<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a>]) -&gt; <a class=\"enum\" href=\"https://doc.rust-lang.org/nightly/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;(<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a>, &amp;[<a class=\"primitive\" href=\"https://doc.rust-lang.org/nightly/std/primitive.u8.html\">u8</a>]), <a class=\"enum\" href=\"x11rb/xcb_ffi/enum.ParseError.html\" title=\"enum x11rb::xcb_ffi::ParseError\">ParseError</a>&gt;</h4></section></summary><div class='docblock'>Try to parse the given values into an instance of this type. <a href=\"x11rb/x11_utils/trait.TryParse.html#tymethod.try_parse\">Read more</a></div></details></div></details>","TryParse","x11rb::protocol::xproto::Keycode","x11rb::protocol::xproto::Button","x11rb::protocol::shape::Op","x11rb::protocol::shape::Kind","x11rb::protocol::xinput::KeyCode","x11rb::protocol::xinput::EventTypeBase","x11rb::protocol::xkb::String8"]]]]);
    if (window.register_type_impls) {
        window.register_type_impls(type_impls);
    } else {
        window.pending_type_impls = type_impls;
    }
})()
//{"start":55,"fragment_lengths":[14,13,4865]}