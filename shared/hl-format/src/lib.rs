#![no_std]

//! Canonical on-disc contract shared by the host cooker and PS1 runtime.
//!
//! This crate deliberately contains only constants and small `const` helpers:
//! depending on it does not allocate resident PS1 state or introduce a runtime
//! service. Keeping both sides on this contract makes format drift a compile
//! error instead of a visual/gameplay omission.

pub mod map {
    pub const SCIENTIST_CORPSE_POSES: [&str; 7] = [
        "lying_on_back",
        "lying_on_stomach",
        "dead_sitting",
        "dead_hang",
        "dead_table1",
        "dead_table2",
        "dead_table3",
    ];
    /// Dead props never cross transitions; their carry word stores a map-local
    /// static pose clip instead. The flag distinguishes old cooked corpses.
    pub const PROP_CORPSE_CLIP: u16 = 0x8000;

    pub const MAGIC_HLMA: u32 = u32::from_le_bytes(*b"HLMA");
    pub const MAGIC_HLMB: u32 = u32::from_le_bytes(*b"HLMB");
    pub const MAGIC_HLMC: u32 = u32::from_le_bytes(*b"HLMC");
    pub const MAGIC_HLMD: u32 = u32::from_le_bytes(*b"HLMD");
    pub const MAGIC_HLME: u32 = u32::from_le_bytes(*b"HLME");
    pub const MAGIC_HLMF: u32 = u32::from_le_bytes(*b"HLMF");
    pub const MAGIC_HLMG: u32 = u32::from_le_bytes(*b"HLMG");
    pub const MAGIC_HLMH: u32 = u32::from_le_bytes(*b"HLMH");
    pub const MAGIC_LATEST: u32 = MAGIC_HLMH;
    pub const MAGIC_LATEST_BYTES: [u8; 4] = *b"HLMH";

    pub const LEGACY_HEADER_SIZE: usize = 52;
    pub const HEADER_SIZE: usize = 56;
    pub const HEADER_MAGIC_OFFSET: usize = 0;
    pub const HEADER_VERT_COUNT_OFFSET: usize = 4;
    pub const HEADER_TRI_COUNT_OFFSET: usize = 8;
    pub const HEADER_TEXTURE_COUNT_OFFSET: usize = 12;
    pub const HEADER_FACE_COUNT_OFFSET: usize = 16;
    pub const HEADER_BSP_OFFSET: usize = 20;
    pub const HEADER_CLIP_OFFSET: usize = 24;
    pub const HEADER_ENTITY_OFFSET: usize = 28;
    pub const HEADER_TRAM_OFFSET: usize = 32;
    pub const HEADER_PROP_OFFSET: usize = 36;
    pub const HEADER_SKY_TEXTURE_OFFSET: usize = 40;
    pub const HEADER_NAV_OFFSET: usize = 44;
    pub const HEADER_LOGIC_OFFSET: usize = 48;
    pub const HEADER_WORLD_PIPELINE_OFFSET: usize = 52;

    pub const WORLD_PIPELINE_MAGIC: u32 = u32::from_le_bytes(*b"WRP1");
    pub const WORLD_PIPELINE_VERSION: u16 = 9;
    pub const WORLD_PIPELINE_HEADER_SIZE: usize = 48;
    pub const WORLD_PIPELINE_ARRAY_OFFSETS_OFFSET: usize = 24;
    pub const WORLD_PIPELINE_CELL_SIZE: usize = 20;
    pub const WORLD_PIPELINE_LEAF_RANGE_SIZE: usize = 4;
    pub const WORLD_PIPELINE_LEAF_CELL_SIZE: usize = 2;
    // One aligned cell-local position record: packed XYZ, then the migration
    // source id. A selected cell transforms this contiguous range in one hot
    // run; packet commands use one-byte local indices exactly as the resident
    // scene contract requires.
    pub const WORLD_PIPELINE_VERTEX_SIZE: usize = 8;
    // Packet-ready GT4 command: four cell-local u8 vertex indices, four
    // light-palette indices, packed UVs, texture/policy bytes, then
    // migration-only source ids.
    pub const WORLD_PIPELINE_FACE_SIZE: usize = 24;
    pub const WORLD_PIPELINE_TEMPLATE_SIZE: usize = 0;
    pub const WORLD_PIPELINE_MAX_CELL_FACES: usize = 32;
    pub const WORLD_PIPELINE_MAX_CELL_VERTICES: usize = 255;

    // Native-patch loop-vertex topology flags. Each patch is four 5-byte
    // faceverts whose index word keeps its top two bits for per-slot flags:
    //   slot 3 bit 15 -> this patch is a triangle fallback record
    //   any slot bit 14 -> that slot's boundary edge crosses a retained
    //                      source vertex (a real T-junction)
    pub const LOOPVERT_INDEX_MASK: u16 = 0x3fff;
    pub const LOOPVERT_TRIANGLE_FLAG: u16 = 0x8000;
    pub const LOOPVERT_BLOCKED_EDGE_FLAG: u16 = 0x4000;

    pub const WORLD_CELL_STATIC_OPAQUE: u16 = 1 << 0;
    pub const WORLD_CELL_ORDER_PROVEN: u16 = 1 << 1;
    pub const WORLD_FACE_QUAD: u8 = 1 << 0;
    pub const FACE_WORLD_PIPELINE_OWNED: u16 = 1 << 15;
    pub const FACE_COUNT_MASK: u16 = FACE_WORLD_PIPELINE_OWNED - 1;

    pub const WORLD_TEMPLATE_UV_OFFSET: usize = 0;
    pub const WORLD_TEMPLATE_LIGHT_OFFSET: usize = 8;
    pub const WORLD_TEMPLATE_TEXTURE_OFFSET: usize = 12;
    pub const WORLD_TEMPLATE_FLAGS_OFFSET: usize = 13;
    pub const WORLD_TEMPLATE_SOURCE_FACE_OFFSET: usize = 14;
    pub const WORLD_TEMPLATE_SOURCE_CORNER_OFFSET: usize = 16;

    pub const DYNAMIC_FACE_RECORD_SIZE: usize = 12;
    pub const PLANE_RECORD_SIZE: usize = 10;
    pub const FACE_GROUP_RECORD_SIZE: usize = 2;
    pub const NODE_RECORD_SIZE: usize = 6;
    pub const LEAF_RECORD_SIZE: usize = 8;
    pub const FACE_RECORD_SIZE: usize = 16;
    pub const TRI_RECORD_SIZE: usize = 16;
    pub const LOOP_VERTEX_RECORD_SIZE: usize = 5;
    pub const CLIPNODE_RECORD_SIZE: usize = 6;
    pub const ENTITY_RECORD_SIZE: usize = 56;
    pub const PROP_RECORD_SIZE: usize = 24;
    pub const SPRITE_RECORD_SIZE: usize = 12;
    pub const NAV_NODE_RECORD_SIZE: usize = 18;
    pub const LOGIC_RECORD_SIZE: usize = 64;

    pub const PROP_SPLIT_FORMAT: u32 = 0x8000_0000;
    pub const CLIP_PLANE_TAG_MASK: u16 = 0xc000;
    pub const FACE_GROUP_BITS: u16 = 12;
    pub const FACE_GROUP_MASK: u16 = (1 << FACE_GROUP_BITS) - 1;

    pub const LEAF_MARK_COUNT_MASK: u16 = 0x3fff;
    pub const LEAF_LIQUID_SHIFT: u32 = 14;
    pub const LEAF_LIQUID_NONE: u8 = 0;
    pub const LEAF_LIQUID_WATER: u8 = 1;
    pub const LEAF_LIQUID_SLIME: u8 = 2;
    pub const LEAF_LIQUID_LAVA: u8 = 3;

    #[inline(always)]
    pub const fn header_size(magic: u32) -> usize {
        if magic == MAGIC_HLMH {
            HEADER_SIZE
        } else {
            LEGACY_HEADER_SIZE
        }
    }

    #[inline(always)]
    pub const fn supports_tagged_clip_planes(magic: u32) -> bool {
        matches!(
            magic,
            MAGIC_HLMB
                | MAGIC_HLMC
                | MAGIC_HLMD
                | MAGIC_HLME
                | MAGIC_HLMF
                | MAGIC_HLMG
                | MAGIC_HLMH
        )
    }

    #[inline(always)]
    pub const fn supports_split_leaf_counts(magic: u32) -> bool {
        matches!(
            magic,
            MAGIC_HLMC | MAGIC_HLMD | MAGIC_HLME | MAGIC_HLMF | MAGIC_HLMG | MAGIC_HLMH
        )
    }

    #[inline(always)]
    pub const fn supports_texture_animation(magic: u32) -> bool {
        matches!(magic, MAGIC_HLME | MAGIC_HLMF | MAGIC_HLMG | MAGIC_HLMH)
    }

    #[inline(always)]
    pub const fn supports_dynamic_lightmaps(magic: u32) -> bool {
        matches!(magic, MAGIC_HLMG | MAGIC_HLMH)
    }

    #[inline(always)]
    pub const fn supports_world_pipeline(magic: u32) -> bool {
        magic == MAGIC_HLMH
    }

    #[inline(always)]
    pub const fn pack_leaf_mark_count(mark_count: u16, liquid: u8) -> Option<u16> {
        if mark_count > LEAF_MARK_COUNT_MASK || liquid > LEAF_LIQUID_LAVA {
            None
        } else {
            Some(mark_count | ((liquid as u16) << LEAF_LIQUID_SHIFT))
        }
    }

    #[inline(always)]
    pub const fn leaf_mark_count(packed: u16) -> u16 {
        packed & LEAF_MARK_COUNT_MASK
    }

    #[inline(always)]
    pub const fn leaf_liquid(packed: u16) -> u8 {
        (packed >> LEAF_LIQUID_SHIFT) as u8
    }
}

pub mod logic {
    pub const BRUSH_NONE: u16 = u16::MAX;

    pub const FUNC_DOOR: u8 = 1;
    pub const FUNC_BUTTON: u8 = 2;
    pub const TRIGGER_ONCE: u8 = 3;
    pub const TRIGGER_MULTIPLE: u8 = 4;
    pub const TRIGGER_RELAY: u8 = 5;
    pub const MULTI_MANAGER: u8 = 6;
    pub const TRIGGER_AUTO: u8 = 7;
    pub const TRIGGER_CHANGELEVEL: u8 = 8;
    pub const INFO_LANDMARK: u8 = 9;
    pub const TRIGGER_COUNTER: u8 = 10;
    pub const TRIGGER_CHANGETARGET: u8 = 11;
    pub const ITEM_SUIT: u8 = 12;
    pub const ITEM_BATTERY: u8 = 13;
    pub const TRIGGER_HURT: u8 = 14;
    pub const FUNC_TRACKTRAIN: u8 = 15;
    pub const FUNC_BREAKABLE: u8 = 16;
    pub const TRIGGER_TELEPORT: u8 = 17;
    pub const TRIGGER_PUSH: u8 = 18;
    pub const TRIGGER_GRAVITY: u8 = 19;
    pub const HEALTH_CHARGER: u8 = 20;
    pub const HEV_CHARGER: u8 = 21;
    pub const MONSTERMAKER: u8 = 22;
    pub const WEAPON_PICKUP: u8 = 23;
    pub const SCRIPTED: u8 = 24;
    pub const SCRIPTED_HAS_IDLE: u8 = 0x80;
    pub const SCRIPTED_HAS_PLAY: u8 = 0x40;
    /// `trigger_hurt` record flag: GoldSrc's RadiationThink drives the Geiger
    /// counter from this volume even while the player is outside it.
    pub const TRIGGER_HURT_RADIATION: u8 = 0x01;
    pub const FUNC_TRAIN: u8 = 25;
    pub const WEAPONSTRIP: u8 = 26;
    pub const ENV_MESSAGE: u8 = 27;
    pub const ENV_FADE: u8 = 28;
    pub const MAP_FLAGS: u8 = 29;
    pub const CDTRACK: u8 = 30;
    pub const SENTENCE: u8 = 31;
    pub const AMBIENT: u8 = 32;
    pub const ENV_SHAKE: u8 = 33;
    pub const WALL_TOGGLE: u8 = 34;
    pub const MULTISOURCE: u8 = 35;
    pub const ENV_GLOBAL: u8 = 36;
    pub const ENV_EXPLOSION: u8 = 37;
    pub const TANK: u8 = 38;
    pub const BEAM: u8 = 39;
    pub const ENV_SPARK: u8 = 40;
    pub const MONSTERCLIP: u8 = 41;
    pub const MOMENTARY: u8 = 42;
    pub const TRIGGER_TRANSITION: u8 = 43;
    pub const FUNC_ROTATING: u8 = 44;
    pub const FUNC_PENDULUM: u8 = 45;
    pub const MAP_SOUND: u8 = 46;
    pub const WALL_FRAME: u8 = 47;
    pub const FUNC_GUNTARGET: u8 = 48;
    pub const TRIGGER_ENDSECTION: u8 = 49;
    pub const ENV_RENDER: u8 = 50;
    pub const LIGHTSTYLE: u8 = 51;
    pub const ENV_BEVERAGE: u8 = 52;

    pub const USE_OFF: u8 = 0;
    pub const USE_ON: u8 = 1;
    pub const USE_TOGGLE: u8 = 3;

    // Stable public names retained by the runtime and cooker. The shorter
    // names above make the dense-ID invariant readable in this one canonical
    // definition; consumers import only these explicit semantic aliases.
    pub const LOGIC_BRUSH_NONE: u16 = BRUSH_NONE;
    pub const LOGIC_FUNC_DOOR: u8 = FUNC_DOOR;
    pub const LOGIC_FUNC_BUTTON: u8 = FUNC_BUTTON;
    pub const LOGIC_TRIGGER_ONCE: u8 = TRIGGER_ONCE;
    pub const LOGIC_TRIGGER_MULTIPLE: u8 = TRIGGER_MULTIPLE;
    pub const LOGIC_TRIGGER_RELAY: u8 = TRIGGER_RELAY;
    pub const LOGIC_MULTI_MANAGER: u8 = MULTI_MANAGER;
    pub const LOGIC_TRIGGER_AUTO: u8 = TRIGGER_AUTO;
    pub const LOGIC_TRIGGER_CHANGELEVEL: u8 = TRIGGER_CHANGELEVEL;
    pub const LOGIC_INFO_LANDMARK: u8 = INFO_LANDMARK;
    pub const LOGIC_TRIGGER_COUNTER: u8 = TRIGGER_COUNTER;
    pub const LOGIC_TRIGGER_CHANGETARGET: u8 = TRIGGER_CHANGETARGET;
    pub const LOGIC_ITEM_SUIT: u8 = ITEM_SUIT;
    pub const LOGIC_ITEM_BATTERY: u8 = ITEM_BATTERY;
    pub const LOGIC_TRIGGER_HURT: u8 = TRIGGER_HURT;
    pub const LOGIC_TRIGGER_HURT_RADIATION: u8 = TRIGGER_HURT_RADIATION;
    pub const LOGIC_FUNC_TRACKTRAIN: u8 = FUNC_TRACKTRAIN;
    pub const LOGIC_FUNC_BREAKABLE: u8 = FUNC_BREAKABLE;
    pub const LOGIC_TRIGGER_TELEPORT: u8 = TRIGGER_TELEPORT;
    pub const LOGIC_TRIGGER_PUSH: u8 = TRIGGER_PUSH;
    pub const LOGIC_TRIGGER_GRAVITY: u8 = TRIGGER_GRAVITY;
    pub const LOGIC_HEALTH_CHARGER: u8 = HEALTH_CHARGER;
    pub const LOGIC_HEV_CHARGER: u8 = HEV_CHARGER;
    pub const LOGIC_MONSTERMAKER: u8 = MONSTERMAKER;
    pub const LOGIC_WEAPON_PICKUP: u8 = WEAPON_PICKUP;
    pub const LOGIC_SCRIPTED: u8 = SCRIPTED;
    pub const LOGIC_SCRIPTED_HAS_IDLE: u8 = SCRIPTED_HAS_IDLE;
    pub const LOGIC_SCRIPTED_HAS_PLAY: u8 = SCRIPTED_HAS_PLAY;
    pub const LOGIC_FUNC_TRAIN: u8 = FUNC_TRAIN;
    pub const LOGIC_WEAPONSTRIP: u8 = WEAPONSTRIP;
    pub const LOGIC_ENV_MESSAGE: u8 = ENV_MESSAGE;
    pub const LOGIC_ENV_FADE: u8 = ENV_FADE;
    pub const LOGIC_MAP_FLAGS: u8 = MAP_FLAGS;
    pub const LOGIC_CDTRACK: u8 = CDTRACK;
    pub const LOGIC_SENTENCE: u8 = SENTENCE;
    pub const LOGIC_AMBIENT: u8 = AMBIENT;
    pub const LOGIC_ENV_SHAKE: u8 = ENV_SHAKE;
    pub const LOGIC_WALL_TOGGLE: u8 = WALL_TOGGLE;
    pub const LOGIC_MULTISOURCE: u8 = MULTISOURCE;
    pub const LOGIC_ENV_GLOBAL: u8 = ENV_GLOBAL;
    pub const LOGIC_ENV_EXPLOSION: u8 = ENV_EXPLOSION;
    pub const LOGIC_TANK: u8 = TANK;
    pub const LOGIC_BEAM: u8 = BEAM;
    pub const LOGIC_ENV_SPARK: u8 = ENV_SPARK;
    pub const LOGIC_MONSTERCLIP: u8 = MONSTERCLIP;
    pub const LOGIC_MOMENTARY: u8 = MOMENTARY;
    pub const LOGIC_TRIGGER_TRANSITION: u8 = TRIGGER_TRANSITION;
    pub const LOGIC_FUNC_ROTATING: u8 = FUNC_ROTATING;
    pub const LOGIC_FUNC_PENDULUM: u8 = FUNC_PENDULUM;
    pub const LOGIC_MAP_SOUND: u8 = MAP_SOUND;
    pub const LOGIC_WALL_FRAME: u8 = WALL_FRAME;
    pub const LOGIC_FUNC_GUNTARGET: u8 = FUNC_GUNTARGET;
    pub const LOGIC_TRIGGER_ENDSECTION: u8 = TRIGGER_ENDSECTION;
    pub const LOGIC_ENV_RENDER: u8 = ENV_RENDER;
    pub const LOGIC_LIGHTSTYLE: u8 = LIGHTSTYLE;
    pub const LOGIC_ENV_BEVERAGE: u8 = ENV_BEVERAGE;

    pub const KINDS: [u8; 52] = [
        FUNC_DOOR,
        FUNC_BUTTON,
        TRIGGER_ONCE,
        TRIGGER_MULTIPLE,
        TRIGGER_RELAY,
        MULTI_MANAGER,
        TRIGGER_AUTO,
        TRIGGER_CHANGELEVEL,
        INFO_LANDMARK,
        TRIGGER_COUNTER,
        TRIGGER_CHANGETARGET,
        ITEM_SUIT,
        ITEM_BATTERY,
        TRIGGER_HURT,
        FUNC_TRACKTRAIN,
        FUNC_BREAKABLE,
        TRIGGER_TELEPORT,
        TRIGGER_PUSH,
        TRIGGER_GRAVITY,
        HEALTH_CHARGER,
        HEV_CHARGER,
        MONSTERMAKER,
        WEAPON_PICKUP,
        SCRIPTED,
        FUNC_TRAIN,
        WEAPONSTRIP,
        ENV_MESSAGE,
        ENV_FADE,
        MAP_FLAGS,
        CDTRACK,
        SENTENCE,
        AMBIENT,
        ENV_SHAKE,
        WALL_TOGGLE,
        MULTISOURCE,
        ENV_GLOBAL,
        ENV_EXPLOSION,
        TANK,
        BEAM,
        ENV_SPARK,
        MONSTERCLIP,
        MOMENTARY,
        TRIGGER_TRANSITION,
        FUNC_ROTATING,
        FUNC_PENDULUM,
        MAP_SOUND,
        WALL_FRAME,
        FUNC_GUNTARGET,
        TRIGGER_ENDSECTION,
        ENV_RENDER,
        LIGHTSTYLE,
        ENV_BEVERAGE,
    ];
}

#[cfg(test)]
mod tests {
    use super::{logic, map};

    #[test]
    fn latest_format_is_supported_by_every_version_gate() {
        assert!(map::supports_tagged_clip_planes(map::MAGIC_LATEST));
        assert!(map::supports_split_leaf_counts(map::MAGIC_LATEST));
        assert!(map::supports_texture_animation(map::MAGIC_LATEST));
        assert!(map::supports_dynamic_lightmaps(map::MAGIC_LATEST));
        assert!(map::supports_world_pipeline(map::MAGIC_LATEST));
        assert_eq!(map::header_size(map::MAGIC_LATEST), map::HEADER_SIZE);
        assert_eq!(map::header_size(map::MAGIC_HLMG), map::LEGACY_HEADER_SIZE);
    }

    #[test]
    fn leaf_count_and_liquid_share_the_word_without_overlap() {
        for liquid in map::LEAF_LIQUID_NONE..=map::LEAF_LIQUID_LAVA {
            let packed = map::pack_leaf_mark_count(1234, liquid).unwrap();
            assert_eq!(map::leaf_mark_count(packed), 1234);
            assert_eq!(map::leaf_liquid(packed), liquid);
        }
        assert!(map::pack_leaf_mark_count(map::LEAF_MARK_COUNT_MASK + 1, 0).is_none());
        assert!(map::pack_leaf_mark_count(0, map::LEAF_LIQUID_LAVA + 1).is_none());
    }

    #[test]
    fn world_pipeline_face_ownership_preserves_the_legacy_count() {
        assert_eq!(
            map::FACE_WORLD_PIPELINE_OWNED | map::FACE_COUNT_MASK,
            u16::MAX
        );
        assert_eq!(
            (37 | map::FACE_WORLD_PIPELINE_OWNED) & map::FACE_COUNT_MASK,
            37
        );
    }

    #[test]
    fn logic_kind_ids_are_dense_and_unique() {
        for (index, kind) in logic::KINDS.iter().copied().enumerate() {
            assert_eq!(kind as usize, index + 1);
        }
    }
}
