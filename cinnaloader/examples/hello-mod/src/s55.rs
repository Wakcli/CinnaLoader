//! Section 55 demo.

pub fn run() -> Vec<String> {
    let mut map = cinnaloader::serialization::Map::new();
    map.insert("name", cinnaloader::serialization::Value::str("stone"));
    map.insert("count", cinnaloader::serialization::Value::Int(-3));
    map.insert("ratio", cinnaloader::serialization::Value::Float(1.5));
    map.insert(
        "list",
        cinnaloader::serialization::Value::List(vec![
            cinnaloader::serialization::Value::Int(1),
            cinnaloader::serialization::Value::Bool(true),
            cinnaloader::serialization::Value::Null,
        ]),
    );
    let value = cinnaloader::serialization::Value::Map(map);

    let json = cinnaloader::serialization::to_json(&value).expect("finite floats");
    let back = cinnaloader::serialization::from_json(&json).expect("well formed");
    let text = cinnaloader::serialization::to_text(&value);
    let text_back = cinnaloader::serialization::from_text(&text).expect("well formed");

    let binary = cinnaloader::serialization::to_binary(&value);
    let bin_back = cinnaloader::serialization::from_binary(&binary).expect("a whole value");
    let truncated = cinnaloader::serialization::from_binary(&binary[..binary.len() - 1]);

    let malformed = cinnaloader::serialization::from_json(r#"{"a":}"#);
    let duplicate = cinnaloader::serialization::from_json(r#"{"a":1,"a":2}"#).expect("well formed");

    fn bump(
        held: cinnaloader::serialization::Value,
    ) -> Result<cinnaloader::serialization::Value, cinnaloader::serialization::Error> {
        Ok(match held {
            cinnaloader::serialization::Value::Int(number) => {
                cinnaloader::serialization::Value::Int(number + 1)
            }
            other => other,
        })
    }
    cinnaloader::serialization::register_schema(
        cinnaloader::serialization::Schema::new("s55demo")
            .version(2)
            .migration(cinnaloader::serialization::Migration::new(
                1, 2, "bumps an int", bump,
            )),
    );
    let migrated = cinnaloader::serialization::migrate(
        "s55demo",
        1,
        2,
        cinnaloader::serialization::Value::Int(41),
    );
    let missing = cinnaloader::serialization::migrate(
        "s55demo",
        2,
        3,
        cinnaloader::serialization::Value::Int(41),
    );

    let base64 = cinnaloader::serialization::encode_base64(&binary);
    let base64_back = cinnaloader::serialization::decode_base64(&base64).expect("valid base64");
    let bad_base64 = cinnaloader::serialization::decode_base64("aGk*");
    let hex = cinnaloader::serialization::encode_hex(&binary);
    let hex_back = cinnaloader::serialization::decode_hex(&hex).expect("valid hex");
    let bad_hex = cinnaloader::serialization::decode_hex("abc");

    let checksum = cinnaloader::serialization::checksum(&binary);
    let hash = cinnaloader::serialization::hash(&binary);
    let crc = cinnaloader::serialization::checksum(b"123456789");
    let fnv = cinnaloader::serialization::hash(b"");

    let packed = cinnaloader::serialization::compress(binary.as_slice());
    let unpacked = cinnaloader::serialization::decompress(&packed).expect("a whole stream");
    let bad_stream = cinnaloader::serialization::decompress(&packed[..packed.len() - 1]);

    let refused_nbt = cinnaloader::serialization::to_nbt(&value);
    let refused_level = cinnaloader::serialization::from_game_data(&binary);
    let refusal_names: Vec<String> = cinnaloader::serialization::refusals()
        .into_iter()
        .map(|(what, _)| what)
        .collect();
    let why = cinnaloader::serialization::why_unknown("to_nbt");

    let schemas_ok = cinnaloader::serialization::get_schema("s55demo").is_some();
    let json_serializer = cinnaloader::serialization::serialize("json", &value);
    let unknown_serializer = cinnaloader::serialization::serialize("nope", &value);

    vec![
        format!("s55 json {} bytes = {}", json.len(), json),
        format!(
            "s55 json round trip ok={} key order ={:?}",
            back == value,
            value.as_map().and_then(|held| held.keys().next()),
        ),
        format!("s55 text {} bytes = {}", text.len(), text),
        format!("s55 text round trip ok={}", text_back == value),
        format!(
            "s55 binary {} bytes tag=0x{:02x} last=0x{:02x} round trip ok={}",
            binary.len(),
            binary[0],
            binary[binary.len() - 1],
            bin_back == value,
        ),
        format!("s55 truncation refused = {:?}", truncated),
        format!("s55 malformed refused = {:?}", malformed),
        format!(
            "s55 duplicate key len={:?} value={:?}",
            duplicate.as_map().map(cinnaloader::serialization::Map::len),
            duplicate.as_map().and_then(|held| held.get("a")),
        ),
        format!(
            "s55 migrate 1->2 = {:?}; missing 2->3 = {:?}",
            migrated.as_ref().map(|held| format!("{held}")),
            missing.as_ref().map_err(|error| format!("{error}")),
        ),
        format!("s55 schema registered = {}", schemas_ok),
        format!(
            "s55 base64 {} chars = {} round trip ok={}",
            base64.len(),
            base64,
            base64_back == binary,
        ),
        format!("s55 base64 bad input = {:?}", bad_base64),
        format!(
            "s55 hex {} chars = {} round trip ok={}",
            hex.len(),
            hex,
            hex_back == binary,
        ),
        format!("s55 hex bad input = {:?}", bad_hex),
        format!("s55 checksum=0x{:08x} hash=0x{:016x}", checksum, hash),
        format!("s55 crc32(123456789)=0x{crc:08x} fnv1a64(empty)=0x{fnv:016x}"),
        format!(
            "s55 compress {} -> {} bytes round trip ok={}",
            binary.len(),
            packed.len(),
            unpacked == binary,
        ),
        format!("s55 packbits truncation refused = {:?}", bad_stream),
        format!("s55 serialize(json) ok={}", json_serializer.is_ok()),
        format!("s55 serialize(nope) = {:?}", unknown_serializer),
        format!("s55 refused nbt = {:?}", refused_nbt),
        format!("s55 refused level = {:?}", refused_level),
        format!("s55 refusal names = {:?}", refusal_names),
        format!("s55 why_unknown(to_nbt) = {why}"),
    ]
}

