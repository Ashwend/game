# Worlds And Saves

`WorldSave` is JSON: id, name, map, created time, admins, and saved state.

`MapType::Test` builds `WorldData::test_world()`: one floor plus AABB blocks. `Procedural` currently maps to the test world.

`WorldStore::platform_default()` stores saves under the platform app-data directory in `worlds/`.

Singleplayer loads a selected save and persists `last_authoritative_tick` on shutdown. Dedicated server loads `--world` or creates/reuses `Dedicated`.
