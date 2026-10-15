//! Section 76 demo: documents, schemas, dependency order, reload and unload.

pub fn run() -> Vec<String> {
    use cinnaloader::config::{ConfigType, ConfigValue};
    use cinnaloader::datadriven::{self, Document, DocumentSchema, SchemaField};
    let mut out = Vec::new();

    fn object(pairs: &[(&str, ConfigValue)]) -> ConfigValue {
        ConfigValue::Object(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    cinnaloader::datadriven::register_schema(
        DocumentSchema::new("s76:loot")
            .field(
                SchemaField::new("pools", ConfigType::List).required().element(
                    SchemaField::new("entry", ConfigType::Object)
                        .field(SchemaField::new("weight", ConfigType::Integer).required()),
                ),
            )
            .required("rolls", ConfigType::Integer),
    );
    let bad = object(&[(
        "pools",
        ConfigValue::List(vec![object(&[("weight", ConfigValue::Text("heavy".to_owned()))])]),
    )]);
    let problems = datadriven::validate("s76:loot", &bad);
    out.push(format!(
        "datadriven(76): validate refused {} problems at once = [{}] [{}]",
        problems.len(),
        problems[0],
        problems[1]
    ));

    datadriven::register_schema(DocumentSchema::new("s76:item").required("v", ConfigType::Integer));
    let parent = Document::new("demo:red", "s76:item", object(&[("v", ConfigValue::Integer(0))]));
    let child = Document::new("demo:green", "s76:item", object(&[("v", ConfigValue::Integer(0))]))
        .depends_on("demo:red");
    let order = datadriven::load(&[child, parent]).expect("parents load first");
    out.push(format!("datadriven(76): dependency order enforced = {order:?}"));

    let red = Document::new("cyc:red", "s76:item", object(&[])).depends_on("cyc:green");
    let green = Document::new("cyc:green", "s76:item", object(&[])).depends_on("cyc:red");
    let cycle = datadriven::load(&[red, green]).expect_err("a cycle is refused");
    out.push(format!("datadriven(76): cycle refused = {cycle}"));

    let gem = Document::new("demo:gem", "s76:item", object(&[("v", ConfigValue::Integer(1))]));
    let ore = Document::new("demo:ore", "s76:item", object(&[("v", ConfigValue::Integer(2))]));
    datadriven::load(&[gem, ore]).expect("both valid");
    datadriven::reload(Document::new("demo:gem", "s76:item", object(&[("v", ConfigValue::Integer(9))])))
        .expect("valid reload");
    out.push(format!(
        "datadriven(76): reload replaced gem={:?} neighbour ore={:?}",
        datadriven::get("s76:item", "demo:gem"),
        datadriven::get("s76:item", "demo:ore")
    ));

    let refused = datadriven::reload(Document::new("demo:gem", "s76:item", ConfigValue::Text("bad".to_owned())));
    out.push(format!(
        "datadriven(76): refused reload ({}) left old gem={:?}",
        refused.is_err(),
        datadriven::get("s76:item", "demo:gem")
    ));

    out.push(format!("datadriven(76): unload removed gem={} ore kept={:?}", datadriven::unload("demo:gem"), datadriven::get("s76:item", "demo:ore")));
    out.push(format!("datadriven(76): {}", datadriven::summary()));

    out
}
