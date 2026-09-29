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
  view_proj(): Float32Array;
  light_view_proj(): Float32Array;
  prepare_frame(): void;
  grid_width(): number;
  grid_height(): number;
  actual_speed(): number;
  alpha(): number;
  sun(): number;
  moisture(): number;
  light_level(): number;
  mast_year(): boolean;
  diversity(): number;
  local_diversity(): number;
  flooding(): boolean;
  eye(): Float32Array;
  pick_tile(bx: number, by: number): number;
  paint_at(bx: number, by: number, brush: number, species: number, grass: number): number;
  prepare_frame(): void;
  frame_bytes(): Uint8Array;
  frame_counts(): Uint32Array;
  heat(): number;
  set_roots_view(on: boolean): void;
  roots_view(): boolean;
  inspect_at(bx: number, by: number): string;
  deaths_recent(): Float32Array;
  cloud_counts(): Uint32Array;
  cloud_events(): Uint32Array;
  atmosphere(): Float32Array;
  set_biome_view(on: boolean): void;
  set_flashes(on: boolean): void;
  biome_shares(): Float32Array;
  biome_samples(): Float32Array;
};

export type Renderer = {
  free(): void;
  backend(): string;
  resize(w: number, h: number): void;
  set_grid(width: number, height: number): void;
  render(
    viewProj: Float32Array,
    alpha: number,
    light: number,
    heat: number,
    eye: Float32Array,
    lightVp: Float32Array,
    atmos: Float32Array,
    bytes: Uint8Array,
    counts: Uint32Array,
    rootsView: boolean,
  ): void;
};

export type EngineModule = {
  default(opts?: { module_or_path?: string }): Promise<unknown>;
  Simulation: new (seed: number) => Sim;
  Renderer: {
    create(canvas: HTMLCanvasElement): Promise<Renderer>;
    create_offscreen(canvas: OffscreenCanvas): Promise<Renderer>;
  };
};
