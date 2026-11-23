/* The resident scene core's jump table: psx-scene-abi's `slot` constants (SDK branch
   lane/disc-scenes-1123, sdk/crates/psx-scene-abi/src/lib.rs), the start of the core window
   0x801FDF00..0x801FFF00 (provisional 8,192 B), 8 B a slot. Linked only by the `scene-core`
   measurement build while the pin carries no psx-scene-abi; psx-core-abi.ld replaces it. */
PROVIDE(__psx_core_attach      = 0x801FDF00);
PROVIDE(psx_core_switch        = 0x801FDF08);
PROVIDE(psx_core_read          = 0x801FDF10);
PROVIDE(psx_core_pause         = 0x801FDF18);
PROVIDE(psx_core_client_attach = 0x801FDF20);
PROVIDE(psx_core_client_detach = 0x801FDF28);
PROVIDE(psx_core_fatal         = 0x801FDF30);
PROVIDE(psx_core_music_yield   = 0x801FDF38);
PROVIDE(psx_core_stream_open   = 0x801FDF40);
PROVIDE(psx_core_stream_sector = 0x801FDF48);
PROVIDE(psx_core_cd            = 0x801FDF50);
PROVIDE(psx_core_set_idle_hook = 0x801FDF58);
