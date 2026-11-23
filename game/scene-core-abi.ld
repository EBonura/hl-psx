/* The resident scene core's jump table (scene-system-1123/CORE-ABI.md section 3 and
   P2-STREAM-CLIENT.md): fixed slot addresses at the start of the core window, 8 B a slot.
   Linked only by the `scene-core` measurement build, before the core crate (psx-core) exists;
   psx-core-abi.ld replaces it. */
PROVIDE(__psx_core_attach      = 0x801FD700);
PROVIDE(psx_core_switch        = 0x801FD708);
PROVIDE(psx_core_read          = 0x801FD710);
PROVIDE(psx_core_pause         = 0x801FD718);
PROVIDE(psx_core_client_attach = 0x801FD720);
PROVIDE(psx_core_client_detach = 0x801FD728);
PROVIDE(psx_core_fatal         = 0x801FD730);
PROVIDE(psx_core_music_yield   = 0x801FD738);
PROVIDE(psx_core_stream_open   = 0x801FD740);
PROVIDE(psx_core_stream_sector = 0x801FD748);
PROVIDE(psx_core_command       = 0x801FD750);
PROVIDE(psx_core_set_idle_hook = 0x801FD758);
