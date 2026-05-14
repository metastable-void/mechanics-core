#[cfg(any(
    feature = "encoding",
    feature = "rand",
    feature = "console",
    feature = "html",
    feature = "url",
    feature = "mime"
))]
use super::*;

#[test]
#[cfg(feature = "encoding")]
fn form_urlencoded_module_roundtrip() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { encode, decode } from "mechanics:form-urlencoded";
            export default function main(_arg) {
                const encoded = encode({ hello: "world test", x: "1+2" });
                const decoded = decode(encoded);
                return { encoded, decoded };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["decoded"]["hello"], json!("world test"));
    assert_eq!(value["decoded"]["x"], json!("1+2"));
    let encoded = value["encoded"].as_str().expect("encoded should be string");
    assert!(encoded.contains("hello=world+test"));
}

#[test]
#[cfg(feature = "encoding")]
fn form_urlencoded_module_encode_is_key_ordered() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { encode } from "mechanics:form-urlencoded";
            export default function main(_arg) {
                return encode({ z: "last", a: "first", m: "middle" });
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    let encoded = value.as_str().expect("encoded should be string");
    assert_eq!(encoded, "a=first&m=middle&z=last");
}

#[test]
#[cfg(feature = "encoding")]
fn base64_module_roundtrip_base64url() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { encode, decode } from "mechanics:base64";
            export default function main(_arg) {
                const raw = new Uint8Array([1, 2, 3, 250, 255]);
                const encoded = encode(raw, "base64url");
                const decoded = decode(encoded, "base64url");
                return { encoded, bytes: Array.from(decoded) };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["bytes"], json!([1, 2, 3, 250, 255]));
    assert!(
        !value["encoded"]
            .as_str()
            .expect("encoded should be string")
            .contains('=')
    );
}

#[test]
#[cfg(feature = "encoding")]
fn hex_module_roundtrip() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { encode, decode } from "mechanics:hex";
            export default function main(_arg) {
                const raw = new Uint8Array([0, 15, 16, 255]);
                const encoded = encode(raw);
                const decoded = decode(encoded);
                return { encoded, bytes: Array.from(decoded) };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["encoded"], json!("000f10ff"));
    assert_eq!(value["bytes"], json!([0, 15, 16, 255]));
}

#[test]
#[cfg(feature = "encoding")]
fn base32_module_roundtrip_base32hex() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { encode, decode } from "mechanics:base32";
            export default function main(_arg) {
                const raw = new Uint8Array([104, 101, 108, 108, 111]);
                const encoded = encode(raw, "base32hex");
                const decoded = decode(encoded, "base32hex");
                return { encoded, bytes: Array.from(decoded) };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["bytes"], json!([104, 101, 108, 108, 111]));
    assert!(
        value["encoded"]
            .as_str()
            .expect("encoded should be string")
            .len()
            >= 8
    );
}

#[test]
#[cfg(feature = "rand")]
fn rand_module_fills_buffer() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import fillRandom from "mechanics:rand";
            export default function main(_arg) {
                const raw = new Uint8Array(32);
                fillRandom(raw);
                const arr = Array.from(raw);
                const anyNonZero = arr.some((x) => x !== 0);
                return { anyNonZero, len: arr.length };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["len"], json!(32));
    assert_eq!(value["anyNonZero"], json!(true));
}

#[test]
#[cfg(feature = "rand")]
fn rand_module_fills_arraybuffer_and_dataview() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import fillRandom from "mechanics:rand";
            export default function main(_arg) {
                const ab = new ArrayBuffer(32);
                const dvBuf = new ArrayBuffer(32);
                const dv = new DataView(dvBuf);
                fillRandom(ab);
                fillRandom(dv);
                const abArr = Array.from(new Uint8Array(ab));
                const dvArr = Array.from(new Uint8Array(dvBuf));
                return {
                    abNonZero: abArr.some((x) => x !== 0),
                    dvNonZero: dvArr.some((x) => x !== 0),
                    abLen: abArr.length,
                    dvLen: dvArr.length,
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["abLen"], json!(32));
    assert_eq!(value["dvLen"], json!(32));
    assert_eq!(value["abNonZero"], json!(true));
    assert_eq!(value["dvNonZero"], json!(true));
}

#[test]
#[cfg(feature = "encoding")]
fn base64_decode_rejects_invalid_input() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { decode } from "mechanics:base64";
            export default function main(_arg) {
                return decode("%%%");
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("invalid base64 input should fail decode");
    match err {
        MechanicsError::Execution(msg) => assert!(msg.to_ascii_lowercase().contains("invalid")),
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
#[cfg(feature = "encoding")]
fn hex_decode_rejects_invalid_input() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { decode } from "mechanics:hex";
            export default function main(_arg) {
                return decode("zz");
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("invalid hex input should fail decode");
    match err {
        MechanicsError::Execution(msg) => assert!(msg.to_ascii_lowercase().contains("invalid")),
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
#[cfg(feature = "encoding")]
fn base32_decode_rejects_invalid_input() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { decode } from "mechanics:base32";
            export default function main(_arg) {
                return decode("***", "base32");
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("invalid base32 input should fail decode");
    match err {
        MechanicsError::Execution(msg) => assert!(msg.to_ascii_lowercase().contains("invalid")),
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
#[cfg(feature = "rand")]
fn uuid_module_supports_core_variants() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import uuid from "mechanics:uuid";
            export default function main(_arg) {
                const nil = uuid("nil");
                const max = uuid("max");
                const v4 = uuid("v4");
                const v6 = uuid("v6");
                const v7 = uuid("v7");
                const ns = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
                const v5a = uuid("v5", { namespace: ns, name: "example" });
                const v5b = uuid("v5", { namespace: ns, name: "example" });
                return { nil, max, v4, v6, v7, v5a, v5b, v5Stable: v5a === v5b };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");

    assert_eq!(value["nil"], json!("00000000-0000-0000-0000-000000000000"));
    assert_eq!(value["max"], json!("ffffffff-ffff-ffff-ffff-ffffffffffff"));
    for key in ["v4", "v6", "v7", "v5a"] {
        let s = value[key].as_str().expect("uuid must be string");
        assert_eq!(s.len(), 36);
        assert_eq!(&s[8..9], "-");
        assert_eq!(&s[13..14], "-");
        assert_eq!(&s[18..19], "-");
        assert_eq!(&s[23..24], "-");
    }
    assert_eq!(value["v5Stable"], json!(true));
}

#[test]
#[cfg(feature = "rand")]
fn uuid_module_rejects_missing_v5_options() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import uuid from "mechanics:uuid";
            export default function main(_arg) {
                return uuid("v5");
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool.run(job).expect_err("missing v5 options should fail");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("options"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
#[cfg(feature = "console")]
fn console_module_methods_are_noop_undefined() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import console from "mechanics:console";
            export default function main(_arg) {
                return {
                    log: console.log(),
                    info: console.info("hello"),
                    warn: console.warn("x", 1, true, null),
                    error: console.error({ code: "E" }, ["a"]),
                    debug: console.debug(undefined, { nested: { ok: true } }),
                    levelCount: ["log", "info", "warn", "error", "debug"]
                        .filter((name) => typeof console[name] === "function").length,
                    hasGlobalConsole: "console" in globalThis,
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["levelCount"], json!(5));
    assert_eq!(value["hasGlobalConsole"], json!(false));
    for key in ["log", "info", "warn", "error", "debug"] {
        assert_eq!(value[key], Value::Null);
    }
}

#[test]
#[cfg(feature = "html")]
fn html_module_escape_and_unescape_roundtrips() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import {
                escapeText,
                escapeAttribute,
                unescapeText,
                unescapeAttribute,
            } from "mechanics:html";

            export default function main(_arg) {
                const raw = "a&b<c>\"'";
                const textEscaped = escapeText(raw);
                const attrEscaped = escapeAttribute(raw);
                return {
                    textEscaped,
                    attrEscaped,
                    textRoundtrip: unescapeText(textEscaped),
                    attrRoundtrip: unescapeAttribute(attrEscaped),
                    textCanonical: unescapeText("&amp;&lt;&gt;&quot;&apos;"),
                    attrCanonical: unescapeAttribute("1&times2&lt;3"),
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["textEscaped"], json!("a&amp;b&lt;c&gt;\"'"));
    assert_eq!(value["attrEscaped"], json!("a&amp;b&lt;c&gt;&quot;&apos;"));
    assert_eq!(value["textRoundtrip"], json!("a&b<c>\"'"));
    assert_eq!(value["attrRoundtrip"], json!("a&b<c>\"'"));
    assert_eq!(value["textCanonical"], json!("&<>\"'"));
    assert_eq!(value["attrCanonical"], json!("1&times2<3"));
}

#[test]
#[cfg(feature = "html")]
fn html_module_rejects_non_string_arg() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { escapeText } from "mechanics:html";
            export default function main(_arg) {
                return escapeText(123);
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let err = pool
        .run(job)
        .expect_err("non-string html input should fail");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("text must be a string"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}

#[test]
#[cfg(feature = "url")]
fn url_module_constructs_and_exposes_accessors() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import URL from "mechanics:url";
            export default function main(_arg) {
                const url = new URL("https://user:pass@example.com:8443/a/b?x=1#frag");
                return {
                    href: url.href,
                    origin: url.origin,
                    protocol: url.protocol,
                    username: url.username,
                    password: url.password,
                    host: url.host,
                    hostname: url.hostname,
                    port: url.port,
                    pathname: url.pathname,
                    search: url.search,
                    hash: url.hash,
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(
        value["href"],
        json!("https://user:pass@example.com:8443/a/b?x=1#frag")
    );
    assert_eq!(value["origin"], json!("https://example.com:8443"));
    assert_eq!(value["protocol"], json!("https:"));
    assert_eq!(value["username"], json!("user"));
    assert_eq!(value["password"], json!("pass"));
    assert_eq!(value["host"], json!("example.com:8443"));
    assert_eq!(value["hostname"], json!("example.com"));
    assert_eq!(value["port"], json!("8443"));
    assert_eq!(value["pathname"], json!("/a/b"));
    assert_eq!(value["search"], json!("?x=1"));
    assert_eq!(value["hash"], json!("#frag"));
}

#[test]
#[cfg(feature = "url")]
fn url_module_supports_base_relative_construction() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import URL from "mechanics:url";
            export default function main(_arg) {
                return new URL("../next?q=1", "https://example.com/a/b/current").href;
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value, json!("https://example.com/a/next?q=1"));
}

#[test]
#[cfg(feature = "url")]
fn url_module_rejects_invalid_input_and_bare_call() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import URL from "mechanics:url";
            export default function main(_arg) {
                let invalid = false;
                let bare = false;
                try { new URL("not relative"); } catch (e) { invalid = e instanceof TypeError; }
                try { URL("https://example.com/"); } catch (e) { bare = e instanceof TypeError; }
                return { invalid, bare };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value, json!({"invalid": true, "bare": true}));
}

#[test]
#[cfg(feature = "url")]
fn url_module_property_mutation_updates_href() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import URL from "mechanics:url";
            export default function main(_arg) {
                const url = new URL("https://example.com/a?x=1#old");
                url.protocol = "http:";
                url.hostname = "example.org";
                url.port = "8080";
                url.pathname = "b/c";
                url.search = "?y=2";
                url.hash = "new";
                return url.href;
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value, json!("http://example.org:8080/b/c?y=2#new"));
}

#[test]
#[cfg(feature = "url")]
fn url_module_search_params_mutations_bind_to_url() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import URL from "mechanics:url";
            export default function main(_arg) {
                const url = new URL("https://example.com/path?z=last&a=old&a=second");
                const params = url.searchParams;
                params.append("m", "middle");
                params.set("a", "first");
                params.delete("z");
                params.sort();
                url.search = "?q=reset";
                params.append("tail", "yes");
                return {
                    href: url.href,
                    search: url.search,
                    params: params.toString(),
                    entries: Array.from(params.entries()),
                    keys: Array.from(params.keys()),
                    values: Array.from(params.values()),
                    spread: Array.from(params),
                    size: params.size,
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(
        value["href"],
        json!("https://example.com/path?q=reset&tail=yes")
    );
    assert_eq!(value["search"], json!("?q=reset&tail=yes"));
    assert_eq!(value["params"], json!("q=reset&tail=yes"));
    assert_eq!(value["entries"], json!([["q", "reset"], ["tail", "yes"]]));
    assert_eq!(value["keys"], json!(["q", "tail"]));
    assert_eq!(value["values"], json!(["reset", "yes"]));
    assert_eq!(value["spread"], json!([["q", "reset"], ["tail", "yes"]]));
    assert_eq!(value["size"], json!(2));
}

#[test]
#[cfg(feature = "url")]
fn url_search_params_supports_string_iterable_and_object_inputs() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { URLSearchParams } from "mechanics:url";
            export default function main(_arg) {
                const fromString = new URLSearchParams("?a=1&a=2+b");
                const fromIterable = new URLSearchParams([["z", "9"], ["a", "1"]]);
                const fromObject = new URLSearchParams({ b: "2", a: "1" });
                const seen = [];
                fromIterable.forEach((value, name, self) => {
                    seen.push([name, value, self === fromIterable]);
                });
                return {
                    string: {
                        first: fromString.get("a"),
                        all: fromString.getAll("a"),
                        hasValue: fromString.has("a", "2 b"),
                        hasMissingValue: fromString.has("a", "missing"),
                        text: fromString.toString(),
                    },
                    iterable: {
                        text: fromIterable.toString(),
                        seen,
                    },
                    object: {
                        entries: Array.from(fromObject.entries()).sort(),
                    },
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["string"]["first"], json!("1"));
    assert_eq!(value["string"]["all"], json!(["1", "2 b"]));
    assert_eq!(value["string"]["hasValue"], json!(true));
    assert_eq!(value["string"]["hasMissingValue"], json!(false));
    assert_eq!(value["string"]["text"], json!("a=1&a=2+b"));
    assert_eq!(value["iterable"]["text"], json!("z=9&a=1"));
    assert_eq!(
        value["iterable"]["seen"],
        json!([["z", "9", true], ["a", "1", true]])
    );
    assert_eq!(value["object"]["entries"], json!([["a", "1"], ["b", "2"]]));
}

#[test]
#[cfg(feature = "url")]
fn url_module_to_string_to_json_and_statics() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import URL from "mechanics:url";
            export default function main(_arg) {
                const url = new URL("/x", "https://example.com/base/");
                const parsed = URL.parse("/y", "https://example.com/base/");
                const failed = URL.parse("/z");
                return {
                    text: url.toString(),
                    json: url.toJSON(),
                    canAbsolute: URL.canParse("https://example.com/"),
                    canRelative: URL.canParse("/rel", "https://example.com/"),
                    cannotRelative: URL.canParse("/rel"),
                    parsed: parsed.href,
                    failed,
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["text"], json!("https://example.com/x"));
    assert_eq!(value["json"], json!("https://example.com/x"));
    assert_eq!(value["canAbsolute"], json!(true));
    assert_eq!(value["canRelative"], json!(true));
    assert_eq!(value["cannotRelative"], json!(false));
    assert_eq!(value["parsed"], json!("https://example.com/y"));
    assert_eq!(value["failed"], Value::Null);
}

#[test]
#[cfg(feature = "mime")]
fn mime_module_composes_simple_text_with_crlf_and_quoted_printable_header() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { compose } from "mechanics:mime";
            export default function main(_arg) {
                const raw = compose({
                    headers: { Subject: "こんにちは" },
                    body: "hello\ncafé",
                });
                return {
                    raw,
                    hasCrLf: raw.includes("\r\n"),
                    noBareLf: !/(^|[^\r])\n/.test(raw),
                    subjectEncoded: raw.includes("Subject: =?UTF-8?Q?"),
                    transfer: raw.includes("Content-Transfer-Encoding: quoted-printable"),
                    bodyEncoded: raw.includes("caf=C3=A9"),
                    mimeVersion: raw.includes("MIME-Version: 1.0\r\n"),
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["hasCrLf"], json!(true));
    assert_eq!(value["noBareLf"], json!(true));
    assert_eq!(value["subjectEncoded"], json!(true));
    assert_eq!(value["transfer"], json!(true));
    assert_eq!(value["bodyEncoded"], json!(true));
    assert_eq!(value["mimeVersion"], json!(true));
}

#[test]
#[cfg(feature = "mime")]
fn mime_module_composes_multipart_and_uint8array_body() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { compose } from "mechanics:mime";
            export default function main(_arg) {
                const raw = compose({
                    headers: { Subject: "Files" },
                    parts: [
                        { headers: { "Content-Type": "text/plain; charset=utf-8" }, body: "plain" },
                        {
                            headers: { "Content-Type": "application/octet-stream" },
                            body: new Uint8Array([0, 1, 2, 250, 255]),
                        },
                    ],
                });
                const boundary = /boundary="([^"]+)"/.exec(raw)[1];
                return {
                    raw,
                    mixed: raw.includes("Content-Type: multipart/mixed; boundary="),
                    firstPart: raw.includes("Content-Transfer-Encoding: 7bit\r\n\r\nplain"),
                    binaryPart: raw.includes("Content-Transfer-Encoding: base64"),
                    encodedBytes: raw.includes("AAEC+v8="),
                    closes: raw.includes("--" + boundary + "--\r\n"),
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["mixed"], json!(true));
    assert_eq!(value["firstPart"], json!(true));
    assert_eq!(value["binaryPart"], json!(true));
    assert_eq!(value["encodedBytes"], json!(true));
    assert_eq!(value["closes"], json!(true));
}

#[test]
#[cfg(feature = "mime")]
fn mime_module_parses_simple_and_multipart_messages() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { parse } from "mechanics:mime";
            export default function main(_arg) {
                const simple = parse(
                    "Subject: =?UTF-8?Q?caf=C3=A9?=\n" +
                    "Content-Type: text/plain; charset=utf-8\n" +
                    "Content-Transfer-Encoding: quoted-printable\n\n" +
                    "hello=20caf=C3=A9"
                );
                const multi = parse(
                    "Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n" +
                    "--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\npart one\r\n" +
                    "--b\r\nContent-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\n\r\nAAEC\r\n" +
                    "--b--\r\n"
                );
                return {
                    subject: simple.headers.Subject,
                    simpleBody: simple.body,
                    partCount: multi.parts.length,
                    textPart: multi.parts[0].body,
                    bytes: Array.from(multi.parts[1].body),
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["subject"], json!("café"));
    assert_eq!(value["simpleBody"], json!("hello café"));
    assert_eq!(value["partCount"], json!(2));
    assert_eq!(value["textPart"], json!("part one"));
    assert_eq!(value["bytes"], json!([0, 1, 2]));
}

#[test]
#[cfg(feature = "mime")]
fn mime_module_roundtrips_structured_message() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { compose, parse } from "mechanics:mime";
            export default function main(_arg) {
                const raw = compose({
                    headers: { Subject: "Round trip" },
                    parts: [
                        { body: "hello" },
                        { headers: { "Content-Type": "application/octet-stream" }, body: new Uint8Array([9, 8, 7]) },
                    ],
                });
                const parsed = parse(raw);
                return {
                    subject: parsed.headers.Subject,
                    first: parsed.parts[0].body,
                    second: Array.from(parsed.parts[1].body),
                };
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value["subject"], json!("Round trip"));
    assert_eq!(value["first"], json!("hello"));
    assert_eq!(value["second"], json!([9, 8, 7]));
}

#[test]
#[cfg(feature = "mime")]
fn mime_module_malformed_input_throws_type_error() {
    let pool = MechanicsPool::new(MechanicsPoolConfig {
        worker_count: 1,
        ..Default::default()
    })
    .expect("create pool");

    let source = r#"
            import { parse } from "mechanics:mime";
            export default function main(_arg) {
                try {
                    parse("Subject without colon\r\n\r\nbody");
                } catch (e) {
                    return e instanceof TypeError;
                }
                return false;
            }
        "#;
    let job = make_job(
        source,
        MechanicsConfig::new(HashMap::new()).expect("create config"),
        Value::Null,
    );
    let value = pool.run(job).expect("run module");
    assert_eq!(value, json!(true));
}
