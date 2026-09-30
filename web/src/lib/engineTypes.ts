// The wasm-bindgen surface of the engine as TS types, so the session code is
// typed without importing the generated package (which only exists after
// `npm run wasm:build`).

export type Sim = {
  free(): void;
  advance(realElapsedSecs: number): number;
  play(): void;
  pause(): void;
  is_playing(): boolean;
  set_speed(v: number): void;
  selected_speed(): number;
  single_step(): void;
  tick(): number;
  seed(): number;
  reseed(seed: number): void;
  counts(): Uint32Array;
  set_params(
    grassSeedP: number,
    grassClonalP: number,
    shadeStrength: number,
    treeGrowthP: number,
    treeRange: number,
    treeMaturityAge: number,
    sodFactor: number,
    crowdingP: number,
    grassMeanLife: number,
    treeMeanLife: number,
    fireIgnitionP: number,
    fireSpreadP: number,
    nutrientBoost: number,
    stormRate: number,
    stormLightningP: number,
    climateSwing: number,
    waterTable: number,
    mutationRate: number,
    terrain: number,
    grassNiches: number,
    pestStrength: number,
    browse: number,
    competition: number,
    climateZones: number,
    rivers: number,
    grazing: number,
    physiology: number,
    seasons: number,
    cloudDynamics: number,
    biomes: number,
    seedTreeP: number,
    seedGrassP: number,
    width: number,
    height: number,
  ): void;
  set_viewport(w: number, h: number): void;
  orbit(dyaw: number, dpitch: number): void;
  pan_pixels(dx: number, dy: number): void;
  zoom(factor: number): void;
  reset_camera(): void;
  prepare_frame(): void;
  grid_width(): number;
  grid_height(): number;
  actual_speed(): number;
  sun(): number;
  moisture(): number;
  mast_year(): boolean;
  diversity(): number;
  local_diversity(): number;
  flooding(): boolean;
  pick_tile(bx: number, by: number): number;
  paint_at(bx: number, by: number, brush: number, species: number, grass: number): number;
  frame_bytes(): Uint8Array;
  frame_uniforms(): Float32Array;
  render_flags(): number;
  terrain_version(): number;
  terrain_vertices(): Uint8Array;
  terrain_indices(): Uint32Array;
  terrain_chunks(): Float32Array;
  skirt_vertices(): Uint8Array;
  skirt_indices(): Uint32Array;
  frame_counts(): Uint32Array;
  heat(): number;
  set_roots_view(on: boolean): void;
  roots_view(): boolean;
  inspect_at(bx: number, by: number): string;
  deaths_recent(): Float32Array;
  cloud_counts(): Uint32Array;
  cloud_events(): Uint32Array;
  set_biome_view(on: boolean): void;
  set_flashes(on: boolean): void;
  biome_shares(): Float32Array;
  biome_samples(): Float32Array;
  // Display settings.
  set_hex_columns(on: boolean): void;
  set_landforms(on: boolean): void;
  set_light_mode(mode: number): void;
  set_bloom(on: boolean): void;
  set_detail(detail: number): void;
  set_hex_overlay(on: boolean): void;
  // Walking.
  enter_walk(bx: number, by: number): void;
  exit_walk(): void;
  walking(): boolean;
  set_keys(bits: number): void;
  look(dyaw: number, dpitch: number): void;
  toggle_third_person(): void;
  act(action: number, species: number, grass: number): string;
  inspect_target(): string;
  target_label(): string;
  walk_status(): Float32Array;
  walk_message(): string;
};

export type Renderer = {
  free(): void;
  backend(): string;
  resize(w: number, h: number): void;
  hdr(): boolean;
  set_terrain(
    vertices: Uint8Array,
    indices: Uint32Array,
    chunks: Float32Array,
    skirtVertices: Uint8Array,
    skirtIndices: Uint32Array,
  ): void;
  render(uniforms: Float32Array, bytes: Uint8Array, counts: Uint32Array, flags: number): void;
};

export type EngineModule = {
  default(opts?: { module_or_path?: string }): Promise<unknown>;
  Simulation: new (seed: number) => Sim;
  Renderer: {
    create(canvas: HTMLCanvasElement): Promise<Renderer>;
    create_offscreen(canvas: OffscreenCanvas): Promise<Renderer>;
  };
};
