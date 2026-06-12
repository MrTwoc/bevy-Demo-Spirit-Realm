broses
LZ
 — 2026/6/6 16:50
I'm working on a plugin that duplicates some of the infrastructure from the render world into the main world so I can run compute shaders that are interleaved with FixedUpdate. I know that the general advice is to run compute shaders in the render world, but afaict if I want my game logic to be deterministic and run on both CPU and GPU then this is sorta what I have to do, right? The design I'm trying out is adding a Compute schedule before FixedUpdate for submitting compute graph commands (I'm using bevy 0.19, so the graph is just systems), and then reading back after FixedUpdate. This way FixedUpdate can run in parallel with the compute shaders, and hopefully I can get a similar effect to the main world / render world split.

I have a rough picture of what the plugin will look like, and I'm working on an example using it. I would love to get some feedback on my approach and any problems I'm likely to run into. https://github.com/akriegman/bevy_voxel/blob/main/src/compute.rs

I'm also putting together some notes on all the wgpu / bevy_render types and how they relate to each other. Maybe people will find this useful: https://github.com/akriegman/bevy_voxel/blob/main/notes.md

What I'm stuck on right now is understanding PipelineCache and deciding whether I should use it. From what I can tell, I can just create my pipelines and BindGroupLayouts once when the RenderDevice is acquired, and then I don't need the PipelineCache? And it seems slightly simpler to not use it. So when is the PipelineCache useful?
GitHub
bevy_voxel/src/compute.rs at main · akriegman/bevy_voxel
Contribute to akriegman/bevy_voxel development by creating an account on GitHub.
Contribute to akriegman/bevy_voxel development by creating an account on GitHub.
GitHub
bevy_voxel/notes.md at main · akriegman/bevy_voxel
Contribute to akriegman/bevy_voxel development by creating an account on GitHub.
Contribute to akriegman/bevy_voxel development by creating an account on GitHub.
broses
LZ
 — 2026/6/6 19:09
Short version bc the full thing doesn't fit on discord:
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct Compute;

pub struct ComputePlugin;
impl Plugin for ComputePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingCommandBuffers>();

        app.init_schedule(Compute);
        app.world_mut()
            .resource_mut::<FixedMainScheduleOrder>()
            .insert_before(FixedPreUpdate, Compute);

        app.add_systems(FixedFirst, process_pipeline_queue);
        app.add_systems(FixedPreUpdate, submit);

        app.configure_sets(
            PreUpdate,
            ComputeStartup.run_if(resource_changed::<RenderDevice>),
        );
    }
}

fn process_pipeline_queue(mut pipeline_cache: ResMut<PipelineCache>) {
    pipeline_cache.process_queue();
}

fn submit(mut flush: FlushCommands) {
    flush.flush();
}

/* ------------------ resource initialization ------------------ */

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
struct ComputeStartup;

pub fn init_gpu_resource<R: Resource + FromWorld>(world: &mut World) {
    let res = R::from_world(world);
    world.insert_resource(res);
}

pub trait ComputeResourceAppExt {
    fn init_gpu_resource<R: Resource + FromWorld>(&mut self) -> &mut Self;
}

impl ComputeResourceAppExt for App {
    fn init_gpu_resource<R: Resource + FromWorld>(&mut self) -> &mut Self {
        self.add_systems(
            PreUpdate,
            init_gpu_resource::<R>
                .in_set(ComputeStartup)
                .ambiguous_with_all(),
        )
    }
}
 
nyaalex — 2026/6/7 7:28
in my game I have something similar except it's living in a separate WorldgenWorld, for async mixed CPU/GPU work.
I don't see any issues with your approach. 
As for PipelineCache, I think it's worth using it, because with it hot reloading will work properly and caching of course.
to make code less tedious I recommend writing a helper plugin, in my game I literally call it a ComputePipelinePlugin<T> where T implements a trait, ComputePipelineRecipe. and then it conveniently adds a resource, CachedComputePipeline that you can use in the systems and forget about the PipelineCache.
here's code for reference: https://gist.github.com/nyaalexx/97c884ecd9d0b1cd248849151ea1628f

as for potential issues, the main pain points are if code needs to read or write to an Image asset (because they live in render world), of if you need to do CPU readback (because it's asynchronous)

though if you have some gpu simulation (like liquid, gas, whatever) I'd honestly recommend to try to put it in render schedule. you can easily make it run multiple times per frame or not run at all, mimicking how FixedUpdate works. This approach might be easier.
Gist
example.rs
GitHub Gist: instantly share code, notes, and snippets.
GitHub Gist: instantly share code, notes, and snippets.
also forgot to mention that if you do use the PipelineCache, you have to create your own resource in the main world, and then you'd need to sync the shaders from the render world.
or alternatively request pipelines from the render world and then sync them to the main world. either way it's tedius but worth it for proper hot reloading
broses
LZ
 — 2026/6/7 12:39
Ooh I was thinking there should be a way to smooth over the pipeline cache syntax like that
Hmm i think i can set the image usage to main world? And for readbacks i think i can submit the command before FixedUpdate and just hope that it finishes in time for the next FixedUpdate?
I've realized that putting this in the render world would be fine, but if i need deterministic game logic I'd have to sync the two worlds multiple times per frame. And having a FixedExtract schedule would be a closer parallel to the whole Extract situation
broses
LZ
 — 2026/6/7 12:46
But idk my current approach seems easier for now
nyaalex — 2026/6/7 16:38
setting render usage to main won't help, since it only affects whether image data is kept in ram. but for gpu stuff you'd want to access the vram one, and for that you need RenderAssets<GpuImage>. and this resource lives in the render world
broses
LZ
 — 2026/6/10 2:08
I see. Hopefully if I need GpuImages in the main world I'll be able to just clone them somehow, but I'll cross that bridge when I come to it