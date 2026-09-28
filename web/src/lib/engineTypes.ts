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
    seedTreeP: number,
    seedGrassP: number,
  ): void;
  set_viewport(w: number, h: number): void;
  orbit(dyaw: number, dpitch: number): void;
  pan_pixels(dx: number, dy: number): void;
  zoom(factor: number): void;
  reset_camera(): void;
  view_proj(): Float32Array;
  alpha(): number;
  sun(): number;
  moisture(): number;
  light_level(): number;
  eye(): Float32Array;
  pick_tile(bx: number, by: number): number;
  paint_at(bx: number, by: number, brush: number, species: number): number;
  ground_instances(): Uint8Array;
  ground_instance_count(): number;
  tree_instances(species: number): Uint8Array;
  tree_instance_count(species: number): number;
  grass_instances(): Uint8Array;
  grass_instance_count(): number;
  mushroom_instances(): Uint8Array;
  mushroom_instance_count(): number;
  cloud_instances(): Uint8Array;
  cloud_instance_count(): number;
  bolt_instances(): Uint8Array;
  bolt_instance_count(): number;
};

export type Renderer = {
  free(): void;
  backend(): string;
  resize(w: number, h: number): void;
  render(
    viewProj: Float32Array,
    alpha: number,
    light: number,
    eye: Float32Array,
    ground: Uint8Array,
    groundN: number,
    acacia: Uint8Array,
    acaciaN: number,
    oak: Uint8Array,
    oakN: number,
    pine: Uint8Array,
    pineN: number,
    willow: Uint8Array,
    willowN: number,
    grass: Uint8Array,
    grassN: number,
    mushrooms: Uint8Array,
    mushroomsN: number,
    clouds: Uint8Array,
    cloudsN: number,
    bolts: Uint8Array,
    boltsN: number,
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
