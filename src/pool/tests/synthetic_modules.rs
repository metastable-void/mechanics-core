use super::*;

#[test]
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
    let err = pool
        .run(job)
        .expect_err("missing v5 options should fail");
    match err {
        MechanicsError::Execution(msg) => {
            assert!(msg.contains("options"));
        }
        other => panic!("unexpected error kind: {other}"),
    }
}
