//! Block entities: the typed entity a mod really ticks and saves, and the client's own.
//!
//! A **block entity** here is data a mod owns in this process: an id, a kind, a position, a
//! key/value data map of [`ConfigValue`]s, a tick counter, and an optional inventory of
//! [`ItemStack`]s. Get, set, remove, tick, and a save/load round trip through the crate's own
//! serialization all really happen; [`BlockEntity::tick`] advances the counter and runs a
//! furnace-style fuel-burn.
//!
//! What is *not* reachable is the client's block entities in its world, their inventories,
//! who is viewing one, and the update packets that travel over the client's connection;
//! those answer `None` or `false` with the reason from [`why_unknown`].
//!
//! ```
//! use cinnaloader::block_entities::{self, BlockEntity};
//! use cinnaloader::config::ConfigValue;
//!
//! let mut furnace = BlockEntity::new("hello:furnace", (0, 64, 0));
//! furnace.set_data("fuel", ConfigValue::Integer(2));
//! assert_eq!(furnace.tick(), 1);
//! assert_eq!(furnace.get_data("fuel").and_then(ConfigValue::as_integer), Some(1));
//! assert!(block_entities::deserialize(&block_entities::serialize(&furnace)).is_some());
//! ```

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};
use crate::items::ItemStack;

/// A block position.
pub type Pos = (i32, i32, i32);

/// What a mod declares a block entity to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEntityType {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub block: String,
    pub ticks: bool,
    pub has_inventory: bool,
    pub inventory_size: usize,
    pub renderer: Option<String>,
    pub data_handler: Option<String>,
    pub registered: bool,
}

impl BlockEntityType {
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            block: String::new(),
            ticks: true,
            has_inventory: false,
            inventory_size: 0,
            renderer: None,
            data_handler: None,
            registered: false,
        }
    }

    #[must_use]
    pub fn block(mut self, block: &str) -> Self {
        self.block = block.to_owned();
        self
    }

    #[must_use]
    pub fn ticks(mut self, ticks: bool) -> Self {
        self.ticks = ticks;
        self
    }

    #[must_use]
    pub fn inventory(mut self, size: usize) -> Self {
        self.has_inventory = size > 0;
        self.inventory_size = size;
        self
    }
}

/// A block entity instance a mod owns: data, a tick counter and an optional inventory.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntity {
    id: String,
    kind: String,
    pos: Pos,
    data: BTreeMap<String, ConfigValue>,
    ticks: u64,
    inventory: Option<Vec<Option<ItemStack>>>,
    dirty: bool,
}

impl BlockEntity {
    #[must_use]
    pub fn new(kind: &str, pos: Pos) -> Self {
        Self {
            id: format!("{kind}@{},{},{}", pos.0, pos.1, pos.2),
            kind: kind.to_owned(),
            pos,
            data: BTreeMap::new(),
            ticks: 0,
            inventory: None,
            dirty: false,
        }
    }

    #[must_use]
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_owned();
        self
    }

    #[must_use]
    pub fn with_data(mut self, key: &str, value: ConfigValue) -> Self {
        self.data.insert(key.to_owned(), value);
        self
    }

    #[must_use]
    pub fn with_ticks(mut self, ticks: u64) -> Self {
        self.ticks = ticks;
        self
    }

    #[must_use]
    pub fn with_inventory(mut self, slots: Vec<Option<ItemStack>>) -> Self {
        self.inventory = Some(slots);
        self
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    #[must_use]
    pub fn pos(&self) -> Pos {
        self.pos
    }

    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    #[must_use]
    pub fn data(&self) -> &BTreeMap<String, ConfigValue> {
        &self.data
    }

    #[must_use]
    pub fn get_data(&self, key: &str) -> Option<&ConfigValue> {
        self.data.get(key)
    }

    /// Sets a data value, and answers the one it replaced.
    pub fn set_data(&mut self, key: &str, value: ConfigValue) -> Option<ConfigValue> {
        self.dirty = true;
        self.data.insert(key.to_owned(), value)
    }

    pub fn remove_data(&mut self, key: &str) -> bool {
        let removed = self.data.remove(key).is_some();
        self.dirty |= removed;
        removed
    }

    #[must_use]
    pub fn inventory(&self) -> Option<&[Option<ItemStack>]> {
        self.inventory.as_deref()
    }

    pub fn set_inventory(&mut self, slots: Vec<Option<ItemStack>>) -> &mut Self {
        self.inventory = Some(slots);
        self.dirty = true;
        self
    }

    /// Puts a stack in the first empty slot, and answers whether there was room.
    pub fn add_item(&mut self, stack: ItemStack) -> bool {
        let Some(slots) = self.inventory.as_mut() else {
            return false;
        };
        let Some(index) = slots.iter().position(Option::is_none) else {
            return false;
        };
        slots[index] = Some(stack);
        self.dirty = true;
        true
    }

    /// Takes the stack in a slot, if there is one.
    pub fn remove_item(&mut self, index: usize) -> Option<ItemStack> {
        let slots = self.inventory.as_mut()?;
        let taken = slots.get_mut(index)?.take();
        if taken.is_some() {
            self.dirty = true;
        }
        taken
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Advances the tick counter and runs the fuel logic: one unit of `fuel` burns per tick,
    /// and `lit` says whether it was burning at the start of the tick.
    pub fn tick(&mut self) -> u64 {
        self.ticks += 1;
        let fuel = match self.data.get("fuel") {
            Some(ConfigValue::Integer(amount)) => Some(*amount),
            _ => None,
        };
        if let Some(amount) = fuel {
            self.data
                .insert("lit".to_owned(), ConfigValue::Boolean(amount > 0));
            self.data
                .insert("fuel".to_owned(), ConfigValue::Integer((amount - 1).max(0)));
            self.dirty = true;
        }
        self.ticks
    }

    #[must_use]
    pub fn serialize(&self) -> String {
        let mut out = format!(
            "entity\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            self.id, self.kind, self.pos.0, self.pos.1, self.pos.2, self.ticks, self.dirty
        );
        for (key, value) in &self.data {
            out.push_str(&format!(
                "data\t{key}\t{}\t{}\n",
                value.kind().as_str(),
                value.render()
            ));
        }
        if let Some(slots) = &self.inventory {
            out.push_str(&format!("inventory\t{}\n", slots.len()));
            for (index, slot) in slots.iter().enumerate() {
                if let Some(stack) = slot {
                    out.push_str(&format!(
                        "slot\t{index}\t{}\t{}\t{}\n",
                        stack.item(),
                        stack.count(),
                        stack.aux()
                    ));
                }
            }
        }
        out
    }
}

fn kind_of(name: &str) -> Option<ConfigType> {
    Some(match name {
        "boolean" => ConfigType::Boolean,
        "integer" => ConfigType::Integer,
        "float" => ConfigType::Float,
        "string" => ConfigType::String,
        "text" => ConfigType::Text,
        "enum" => ConfigType::Enum,
        "color" => ConfigType::Color,
        "keybind" => ConfigType::Keybind,
        "file_path" => ConfigType::FilePath,
        "directory_path" => ConfigType::DirectoryPath,
        "resource_id" => ConfigType::ResourceId,
        "item_id" => ConfigType::ItemId,
        "block_id" => ConfigType::BlockId,
        "entity_id" => ConfigType::EntityId,
        "position" => ConfigType::Position,
        "vector2" => ConfigType::Vector2,
        "vector3" => ConfigType::Vector3,
        "range" => ConfigType::Range,
        "list" => ConfigType::List,
        "set" => ConfigType::Set,
        "map" => ConfigType::Map,
        "object" => ConfigType::Object,
        "compound" => ConfigType::Compound,
        "duration" => ConfigType::Duration,
        "percentage" => ConfigType::Percentage,
        _ => return None,
    })
}

fn decode_entity(text: &str) -> Option<BlockEntity> {
    let mut lines = text.lines();
    let mut header = lines.next()?.split('\t');
    if header.next()? != "entity" {
        return None;
    }
    let id = header.next()?.to_owned();
    let kind = header.next()?.to_owned();
    let pos = (
        header.next()?.parse().ok()?,
        header.next()?.parse().ok()?,
        header.next()?.parse().ok()?,
    );
    let ticks = header.next()?.parse().ok()?;
    let dirty = header.next()? == "true";
    let mut entity = BlockEntity {
        id,
        kind,
        pos,
        data: BTreeMap::new(),
        ticks,
        inventory: None,
        dirty,
    };
    for line in lines {
        let Some((tag, rest)) = line.split_once('\t') else {
            continue;
        };
        match tag {
            "data" => {
                let mut fields = rest.splitn(3, '\t');
                let key = fields.next()?.to_owned();
                let kind_name = fields.next()?;
                let value = fields.next().unwrap_or("");
                let kind = kind_of(kind_name)?;
                entity.data.insert(key, ConfigValue::parse(value, kind)?);
            }
            "inventory" => {
                entity.inventory = Some(vec![None; rest.parse().ok()?]);
            }
            "slot" => {
                let mut fields = rest.splitn(4, '\t');
                let index = fields.next()?.parse::<usize>().ok()?;
                let item = fields.next()?.to_owned();
                let count = fields.next()?.parse().ok()?;
                let aux = fields.next().and_then(|text| text.parse().ok()).unwrap_or(0);
                let slots = entity.inventory.as_mut()?;
                if let Some(slot) = slots.get_mut(index) {
                    *slot = Some(ItemStack::new(&item, count).with_aux(aux));
                }
            }
            _ => {}
        }
    }
    Some(entity)
}

fn types() -> &'static Mutex<BTreeMap<String, BlockEntityType>> {
    static TYPES: OnceLock<Mutex<BTreeMap<String, BlockEntityType>>> = OnceLock::new();
    TYPES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_types<T>(body: impl FnOnce(&mut BTreeMap<String, BlockEntityType>) -> T) -> T {
    let mut guard = types().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn store() -> &'static Mutex<BTreeMap<String, BlockEntity>> {
    static STORE: OnceLock<Mutex<BTreeMap<String, BlockEntity>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_store<T>(body: impl FnOnce(&mut BTreeMap<String, BlockEntity>) -> T) -> T {
    let mut guard = store().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn with_entity<T>(
    id: &str,
    body: impl FnOnce(&mut BlockEntity) -> T,
) -> Option<T> {
    with_store(|store| store.get_mut(id).map(body))
}

/// Registers a block entity type, or replaces the one with that id.
pub fn register_type(kind: BlockEntityType) -> bool {
    with_types(|types| types.insert(kind.id.clone(), kind).is_none())
}

/// Forgets a declared block entity type.
pub fn unregister_type(id: &str) -> bool {
    with_types(|types| types.remove(id).is_some())
}

/// A declared block entity type, by id.
#[must_use]
pub fn get_type(id: &str) -> Option<BlockEntityType> {
    with_types(|types| types.get(id).cloned())
}

/// Every declared block entity type, in id order.
#[must_use]
pub fn get_types() -> Vec<BlockEntityType> {
    with_types(|types| types.values().cloned().collect())
}

/// Records the renderer a block entity type uses.
pub fn register_renderer(id: &str, renderer: &str) -> bool {
    with_types(|types| match types.get_mut(id) {
        Some(kind) => {
            kind.renderer = Some(renderer.to_owned());
            true
        }
        None => false,
    })
}

/// Records the data handler a block entity type uses.
pub fn register_data_handler(id: &str, handler: &str) -> bool {
    with_types(|types| match types.get_mut(id) {
        Some(kind) => {
            kind.data_handler = Some(handler.to_owned());
            true
        }
        None => false,
    })
}

/// Makes a block entity of a declared type and stores it, or `None` when the type is not
/// declared.
pub fn create(kind: &str, pos: Pos) -> Option<BlockEntity> {
    let declared = get_type(kind)?;
    let mut entity = BlockEntity::new(kind, pos);
    if declared.has_inventory {
        entity.inventory = Some(vec![None; declared.inventory_size.max(1)]);
    }
    with_store(|store| {
        store.insert(entity.id.clone(), entity.clone());
    });
    Some(entity)
}

/// A stored block entity, by id.
#[must_use]
pub fn get(id: &str) -> Option<BlockEntity> {
    with_store(|store| store.get(id).cloned())
}

/// The stored block entity at a position, if one is there.
#[must_use]
pub fn get_at(pos: Pos) -> Option<BlockEntity> {
    with_store(|store| {
        store
            .values()
            .find(|entity| entity.pos == pos)
            .cloned()
    })
}

/// Every stored block entity, in id order.
#[must_use]
pub fn get_all() -> Vec<BlockEntity> {
    with_store(|store| store.values().cloned().collect())
}

/// Writes a block entity as text.
#[must_use]
pub fn serialize(entity: &BlockEntity) -> String {
    entity.serialize()
}

/// Reads a block entity back from text.
#[must_use]
pub fn deserialize(text: &str) -> Option<BlockEntity> {
    decode_entity(text)
}

/// Serializes a stored block entity, or `None` when there is no such entity.
#[must_use]
pub fn save(id: &str) -> Option<String> {
    get(id).map(|entity| entity.serialize())
}

/// Reads a block entity and stores it.
pub fn load(text: &str) -> Option<BlockEntity> {
    let entity = decode_entity(text)?;
    with_store(|store| {
        store.insert(entity.id.clone(), entity.clone());
    });
    Some(entity)
}

/// Marks a stored block entity dirty.
pub fn mark_dirty(id: &str) -> bool {
    with_entity(id, BlockEntity::mark_dirty).is_some()
}

/// Advances a stored block entity one tick and answers its new counter.
pub fn tick(id: &str) -> Option<u64> {
    with_entity(id, BlockEntity::tick)
}

/// One data value of a stored block entity.
#[must_use]
pub fn get_data(id: &str, key: &str) -> Option<ConfigValue> {
    get(id)?.data.get(key).cloned()
}

/// Sets a data value on a stored block entity, answering whether the entity exists.
pub fn set_data(id: &str, key: &str, value: ConfigValue) -> bool {
    with_entity(id, |entity| {
        entity.set_data(key, value);
    })
    .is_some()
}

/// Removes a data value from a stored block entity.
pub fn remove_data(id: &str, key: &str) -> bool {
    with_entity(id, |entity| entity.remove_data(key)).unwrap_or(false)
}

/// The inventory of a stored block entity, if it has one.
#[must_use]
pub fn get_inventory(id: &str) -> Option<Vec<Option<ItemStack>>> {
    get(id)?.inventory
}

/// Replaces the inventory of a stored block entity.
pub fn set_inventory(id: &str, slots: Vec<Option<ItemStack>>) -> bool {
    with_entity(id, |entity| {
        entity.set_inventory(slots);
    })
    .is_some()
}

/// Puts a stack in a stored block entity's first empty slot.
pub fn add_item(id: &str, stack: ItemStack) -> bool {
    with_entity(id, |entity| entity.add_item(stack)).unwrap_or(false)
}

/// Takes a stack from a stored block entity's slot.
pub fn remove_item(id: &str, index: usize) -> Option<ItemStack> {
    with_entity(id, |entity| entity.remove_item(index)).flatten()
}

/// Why a block-entity question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "block_entity" => {
            "block entities in the client's world are the client's world state, which needs \
             the hooks section 10 lists as never"
                .to_owned()
        }
        "inventory" => {
            "a client block entity's inventory is the client's container state, which lives \
             inside the client"
                .to_owned()
        }
        "viewers" => "who is viewing a block entity are the client's players, which this host cannot see".to_owned(),
        "update" => {
            "block entity updates travel over the client's connection; nothing here sends or \
             receives them"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about the client's block entities.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["block_entity", "inventory", "viewers", "update"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The players viewing a stored block entity, which this host cannot see.
#[must_use]
pub fn get_viewers(id: &str) -> Vec<String> {
    crate::warn!(
        "a mod asked this host who is viewing {id}: {}",
        why_unknown("viewers")
    );
    Vec::new()
}

/// Sends a block entity update to the client, which this host cannot do.
pub fn send_update(id: &str) -> bool {
    crate::warn!(
        "a mod asked this host to send an update for {id}: {}",
        why_unknown("update")
    );
    false
}

/// Receives a block entity update from the client, which this host cannot do.
#[must_use]
pub fn receive_update(payload: &str) -> Option<BlockEntity> {
    crate::warn!(
        "a mod handed this host a {}-byte update from the client: {}",
        payload.len(),
        why_unknown("update")
    );
    None
}

/// The block-entity half that only the client can answer.
pub mod known_only_to_the_client {
    use super::{why_unknown, BlockEntity};
    use crate::items::ItemStack;

    /// The client's block entity at a position, which this host cannot read.
    #[must_use]
    pub fn get_at(_x: i32, _y: i32, _z: i32) -> Option<BlockEntity> {
        crate::warn!(
            "a mod asked this host for the block entity at a position: {}",
            why_unknown("block_entity")
        );
        None
    }

    /// The client's block entity, which this host cannot read.
    #[must_use]
    pub fn get(_x: i32, _y: i32, _z: i32) -> Option<BlockEntity> {
        super::known_only_to_the_client::get_at(_x, _y, _z)
    }

    /// The client's block entity inventory, which this host cannot read.
    #[must_use]
    pub fn get_inventory(_x: i32, _y: i32, _z: i32) -> Vec<ItemStack> {
        crate::warn!(
            "a mod asked this host for a client block entity's inventory: {}",
            why_unknown("inventory")
        );
        Vec::new()
    }

    /// The players viewing the client's block entity, which this host cannot see.
    #[must_use]
    pub fn get_viewers(_x: i32, _y: i32, _z: i32) -> Vec<String> {
        super::get_viewers("the client's block entity")
    }

    /// Sends a block entity update to the client, which this host cannot do.
    pub fn send_update(_x: i32, _y: i32, _z: i32) -> bool {
        super::send_update("the client's block entity")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_types(|types| types.clear());
        with_store(|store| store.clear());
    }

    fn furnace() -> BlockEntityType {
        BlockEntityType::new("hello:furnace", "Furnace")
            .block("minecraft:furnace")
            .inventory(3)
    }

    #[test]
    fn a_type_registers_and_decides_the_entitys_shape() {
        let _serial = serial();
        fresh();
        assert!(register_type(furnace()));
        assert!(!register_type(furnace()), "the second replaces the first");
        let kind = get_type("hello:furnace").expect("registered");
        assert_eq!(kind.name, "Furnace");
        assert_eq!(kind.block, "minecraft:furnace");
        assert!(kind.has_inventory);
        assert_eq!(kind.inventory_size, 3);
        assert!(kind.ticks);
        assert!(!kind.registered, "the client was never told");
        assert!(register_renderer("hello:furnace", "hello:furnace_renderer"));
        assert!(register_data_handler("hello:furnace", "hello:furnace_data"));
        assert!(!register_renderer("nothing", "x"));
        let kind = get_type("hello:furnace").expect("registered");
        assert_eq!(kind.renderer.as_deref(), Some("hello:furnace_renderer"));
        assert_eq!(kind.data_handler.as_deref(), Some("hello:furnace_data"));
        assert_eq!(get_types().len(), 1);
        assert!(unregister_type("hello:furnace"));
        assert!(!unregister_type("hello:furnace"));
    }

    #[test]
    fn a_block_entity_stores_gets_and_removes_data() {
        let _serial = serial();
        fresh();
        register_type(furnace());
        let created = create("hello:furnace", (0, 64, 0)).expect("declared");
        assert_eq!(created.id(), "hello:furnace@0,64,0");
        assert_eq!(created.inventory().map(<[_]>::len), Some(3));
        assert!(!created.is_dirty());
        assert!(set_data(created.id(), "progress", ConfigValue::Integer(7)));
        assert_eq!(get_data(created.id(), "progress").and_then(|value| value.as_integer()), Some(7));
        assert!(get(created.id()).expect("stored").is_dirty());
        assert!(remove_data(created.id(), "progress"));
        assert!(!remove_data(created.id(), "progress"));
        assert!(get_data(created.id(), "progress").is_none());
        assert!(mark_dirty(created.id()));
        assert!(!set_data("nothing", "x", ConfigValue::Boolean(true)));
        assert!(!mark_dirty("nothing"));
    }

    #[test]
    fn a_tick_advances_the_counter_and_burns_the_fuel() {
        let _serial = serial();
        fresh();
        register_type(furnace());
        let entity = create("hello:furnace", (0, 64, 0)).expect("declared");
        set_data(entity.id(), "fuel", ConfigValue::Integer(3));
        assert_eq!(tick(entity.id()), Some(1));
        assert_eq!(tick(entity.id()), Some(2));
        assert_eq!(tick(entity.id()), Some(3));
        assert_eq!(get_data(entity.id(), "fuel").and_then(|value| value.as_integer()), Some(0));
        assert_eq!(
            get_data(entity.id(), "lit").and_then(|value| value.as_bool()),
            Some(true),
            "it was still burning when the third tick began"
        );
        assert_eq!(tick(entity.id()), Some(4));
        assert_eq!(get_data(entity.id(), "lit").and_then(|value| value.as_bool()), Some(false));
        assert_eq!(get(entity.id()).expect("stored").ticks(), 4);
        assert_eq!(tick("nothing"), None);
    }

    #[test]
    fn an_inventory_holds_and_removes_stacks() {
        let _serial = serial();
        fresh();
        register_type(furnace());
        let entity = create("hello:furnace", (0, 64, 0)).expect("declared");
        assert!(add_item(entity.id(), ItemStack::new("minecraft:coal", 5)));
        assert!(add_item(entity.id(), ItemStack::new("minecraft:sand", 12)));
        let inventory = get_inventory(entity.id()).expect("it has one");
        assert_eq!(inventory.len(), 3);
        assert_eq!(inventory[0].as_ref().map(ItemStack::item), Some("minecraft:coal"));
        assert_eq!(inventory[0].as_ref().map(ItemStack::count), Some(5));
        assert_eq!(inventory[2], None);
        let taken = remove_item(entity.id(), 0).expect("there was one");
        assert_eq!(taken.item(), "minecraft:coal");
        assert!(get_inventory(entity.id()).expect("it has one")[0].is_none());
        assert!(remove_item(entity.id(), 0).is_none());
        assert!(set_inventory(entity.id(), vec![None; 1]));
        assert_eq!(get_inventory(entity.id()).map(|slots| slots.len()), Some(1));
    }

    #[test]
    fn a_block_entity_survives_a_save_and_load_round_trip() {
        let _serial = serial();
        fresh();
        register_type(furnace());
        let source = create("hello:furnace", (3, 70, -4)).expect("declared");
        set_data(source.id(), "progress", ConfigValue::Integer(11));
        set_data(source.id(), "name", ConfigValue::String("Kiln".to_owned()));
        set_data(source.id(), "lit", ConfigValue::Boolean(true));
        add_item(source.id(), ItemStack::new("minecraft:coal", 5));
        add_item(source.id(), ItemStack::new("minecraft:iron_ore", 2).with_aux(3));
        tick(source.id());
        let text = save(source.id()).expect("stored");
        let loaded = deserialize(&text).expect("it round trips");
        assert_eq!(loaded, get(source.id()).expect("stored"));
        assert_eq!(loaded.ticks(), 1);
        assert_eq!(
            loaded.get_data("name").and_then(|value| value.as_text()),
            Some("Kiln")
        );
        assert_eq!(
            loaded.inventory().and_then(|slots| slots[1].as_ref().map(ItemStack::aux)),
            Some(3)
        );
        fresh();
        assert!(load(&text).is_some());
        assert_eq!(get(source.id()), Some(loaded));
        assert!(deserialize("not an entity").is_none());
        assert!(save("nothing").is_none());
        assert!(load("not an entity").is_none());
    }

    #[test]
    fn create_get_and_get_at_find_the_entity() {
        let _serial = serial();
        fresh();
        register_type(furnace());
        register_type(BlockEntityType::new("hello:sign", "Sign").ticks(false));
        assert!(create("hello:furnace", (0, 64, 0)).is_some());
        assert!(create("hello:sign", (5, 64, 5)).is_some());
        assert_eq!(get_all().len(), 2);
        assert_eq!(get_at((0, 64, 0)).map(|entity| entity.kind().to_owned()), Some("hello:furnace".to_owned()));
        assert_eq!(get_at((5, 64, 5)).map(|entity| entity.kind().to_owned()), Some("hello:sign".to_owned()));
        assert!(get_at((9, 9, 9)).is_none());
        assert!(create("nothing", (0, 0, 0)).is_none());
    }

    #[test]
    fn the_clients_block_entities_and_inventories_are_refused() {
        let _serial = serial();
        assert!(known_only_to_the_client::get_at(0, 64, 0).is_none());
        assert!(known_only_to_the_client::get(0, 64, 0).is_none());
        assert!(known_only_to_the_client::get_inventory(0, 64, 0).is_empty());
        assert!(known_only_to_the_client::get_viewers(0, 64, 0).is_empty());
        assert!(!known_only_to_the_client::send_update(0, 64, 0));
        assert!(get_viewers("hello:furnace").is_empty());
        assert!(!send_update("hello:furnace"));
        assert!(receive_update("payload").is_none());
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
