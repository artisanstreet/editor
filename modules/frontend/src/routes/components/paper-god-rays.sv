<script lang="ts" effect>
	import { Effect, Queue } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { MakeScopedAttachmentRunner } from "$lib/lifecycle/scoped-attachment-runner";
	import { shader_config, type ShaderConfig, type SurfaceToken } from "$lib/shader-config";

	const vertex_shader = `#version 300 es
		in vec2 a_position;
		uniform vec2 u_resolution;
		uniform float u_scale;
		uniform float u_rotation;
		uniform float u_offsetX;
		uniform float u_offsetY;
		out vec2 v_objectUV;

		void main() {
			gl_Position = vec4(a_position, 0.0, 1.0);
			vec2 uv = a_position * 0.5;
			uv.x *= u_resolution.x / max(u_resolution.y, 1.0);
			uv += vec2(-u_offsetX, u_offsetY);
			uv /= max(u_scale, 0.01);
			float rotation = u_rotation * 3.14159265358979323846 / 180.0;
			uv = mat2(cos(rotation), sin(rotation), -sin(rotation), cos(rotation)) * uv;
			v_objectUV = uv;
		}
	`;

	/** Adapted from Paper Design's Apache-2.0 God Rays shader. */
	const fragment_shader = `#version 300 es
		precision mediump float;

		uniform float u_time;
		uniform float u_density;
		uniform float u_spotty;
		uniform float u_intensity;
		uniform vec4 u_color1;
		uniform vec4 u_color2;
		uniform vec4 u_color3;
		uniform vec4 u_color4;
		uniform vec4 u_color5;
		uniform vec4 u_colorBack;
		uniform vec4 u_colorBloom;
		uniform float u_bloom;
		uniform float u_midSize;
		uniform float u_midIntensity;

		in vec2 v_objectUV;
		out vec4 fragColor;

		#define TWO_PI 6.28318530718

		float hash21(vec2 p) {
			p = fract(p * vec2(0.3183099, 0.3678794)) + 0.1;
			p += dot(p, p + 19.19);
			return fract(p.x * p.y);
		}

		float valueNoise(vec2 st) {
			vec2 i = floor(st);
			vec2 f = fract(st);
			float a = hash21(i);
			float b = hash21(i + vec2(1.0, 0.0));
			float c = hash21(i + vec2(0.0, 1.0));
			float d = hash21(i + vec2(1.0, 1.0));
			vec2 u = f * f * (3.0 - 2.0 * f);
			return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
		}

		mat2 rotation(float angle) {
			return mat2(cos(angle), sin(angle), -sin(angle), cos(angle));
		}

		float raysShape(vec2 uv, float radius, float frequency, float power) {
			float angle = atan(uv.y, uv.x);
			vec2 left = vec2(angle * frequency, radius);
			vec2 right = vec2(fract(angle / TWO_PI) * TWO_PI * frequency, radius);
			return mix(pow(valueNoise(right), power), pow(valueNoise(left), power), smoothstep(-0.15, 0.15, uv.x));
		}

		vec4 rayColor(int index) {
			if (index == 0) return u_color1;
			if (index == 1) return u_color2;
			if (index == 2) return u_color3;
			if (index == 3) return u_color4;
			return u_color5;
		}

		void main() {
			vec2 uv = v_objectUV;
			float radius = length(uv);
			float time = 0.2 * u_time;
			float spots = 6.5 * abs(u_spotty);
			float power = 4.0 - 3.0 * clamp(u_intensity, 0.0, 1.0);
			float density = 6.0 * u_density + step(0.5, u_density) * pow(4.5 * (u_density - 0.5), 4.0);

			float midSize = 10.0 * abs(u_midSize);
			float middleShape = pow(u_midIntensity, 0.3) * (1.0 - smoothstep(0.02 * midSize, max(midSize, 0.000001), 3.0 * radius));
			middleShape = pow(middleShape, 5.0);

			vec3 accumColor = vec3(0.0);
			float accumAlpha = 0.0;
			for (int i = 0; i < 5; i++) {
				vec2 rotated = rotation(float(i) + 1.0) * uv;
				float r1 = radius * (1.0 + 0.4 * float(i)) - 3.0 * time;
				float r2 = 0.5 * radius * (1.0 + spots) - 2.0 * time;
				float frequency = (1.0 + 0.5 * float(i)) * density;
				float ray = raysShape(rotated, r1, 5.0 * frequency, power);
				ray *= raysShape(rotated, r2, 4.0 * frequency, power);
				ray += (1.0 + 4.0 * ray) * middleShape;
				ray = clamp(ray, 0.0, 1.0);

				vec4 source = rayColor(i);
				float sourceAlpha = source.a * ray;
				vec3 sourceColor = source.rgb * sourceAlpha;
				vec3 alphaColor = accumColor + (1.0 - accumAlpha) * sourceColor;
				float alphaValue = accumAlpha + (1.0 - accumAlpha) * sourceAlpha;
				vec3 addColor = accumColor + sourceColor;
				float addValue = accumAlpha + sourceAlpha;
				accumColor = mix(alphaColor, addColor, u_bloom);
				accumAlpha = mix(alphaValue, addValue, u_bloom);
			}

			vec3 bloomColor = u_colorBloom.rgb * u_colorBloom.a;
			accumColor = mix(accumColor, accumColor + accumAlpha * bloomColor, u_bloom);
			vec3 background = u_colorBack.rgb * u_colorBack.a;
			vec3 color = accumColor + (1.0 - accumAlpha) * background;
			float alpha = accumAlpha + (1.0 - accumAlpha) * u_colorBack.a;
			color += (hash21(gl_FragCoord.xy) - 0.5) / 256.0;
			fragColor = vec4(clamp(color, 0.0, 1.0), clamp(alpha, 0.0, 1.0));
		}
	`;

	const AcquirePaperRays = (canvas: HTMLCanvasElement) =>
		Effect.gen(function* () {
		const gl = yield* RunBrowserDom(() => canvas.getContext("webgl2", {
			alpha: true,
			antialias: false,
			powerPreference: "low-power",
		}));
		if (gl === null) return;
		const releases: Array<Effect.Effect<void>> = [];
		const ReleaseResources = () =>
			Effect.gen(function* () {
			for (const release of releases.reverse()) yield* release;
			});

		const CompileShader = (type: number, source: string) =>
			Effect.gen(function* () {
			return yield* RunBrowserDom(() => {
				const shader = gl.createShader(type);
				if (shader === null) return null;
				gl.shaderSource(shader, source);
				gl.compileShader(shader);
				if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
					gl.deleteShader(shader);
					return null;
				}
				return shader;
			});
			});

		const vertex = yield* CompileShader(gl.VERTEX_SHADER, vertex_shader);
		const fragment = yield* CompileShader(gl.FRAGMENT_SHADER, fragment_shader);
		if (vertex !== null)
			releases.push(
				Effect.gen(function* () {
					yield* RunBrowserDom(() => gl.deleteShader(vertex));
				}),
			);
		if (fragment !== null)
			releases.push(
				Effect.gen(function* () {
					yield* RunBrowserDom(() => gl.deleteShader(fragment));
				}),
			);
		if (vertex === null || fragment === null) return ReleaseResources;

		const program = yield* RunBrowserDom(() => gl.createProgram());
		if (program === null) return ReleaseResources;
		releases.push(
			Effect.gen(function* () {
				yield* RunBrowserDom(() => gl.deleteProgram(program));
			}),
		);
		const linked = yield* RunBrowserDom(() => {
			gl.attachShader(program, vertex);
			gl.attachShader(program, fragment);
			gl.linkProgram(program);
			return gl.getProgramParameter(program, gl.LINK_STATUS);
		});
		if (!linked) return ReleaseResources;
		yield* RunBrowserDom(() => gl.useProgram(program));

		const buffer = yield* RunBrowserDom(() => gl.createBuffer());
		if (buffer === null) return ReleaseResources;
		releases.push(
			Effect.gen(function* () {
				yield* RunBrowserDom(() => gl.deleteBuffer(buffer));
			}),
		);
		const { color_context, resolution, time } = yield* RunBrowserDom(() => {
			gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
			gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]), gl.STATIC_DRAW);
			const position = gl.getAttribLocation(program, "a_position");
			gl.enableVertexAttribArray(position);
			gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);
			const color_canvas = document.createElement("canvas");
			color_canvas.width = 1;
			color_canvas.height = 1;
			return { color_context: color_canvas.getContext("2d", { willReadFrequently: true }), resolution: gl.getUniformLocation(program, "u_resolution"), time: gl.getUniformLocation(program, "u_time") };
		});
		const color_cache = new Map<SurfaceToken, readonly [number, number, number]>();
		const ResolveSurface = (token: SurfaceToken) =>
			Effect.gen(function* () {
			const cached = color_cache.get(token);
			if (cached !== undefined) return cached;
			if (color_context === null) return [0, 0, 0] as const;

			const color = yield* RunBrowserDom(() => {
				const value = getComputedStyle(document.documentElement).getPropertyValue(`--${token}`).trim();
				color_context.clearRect(0, 0, 1, 1);
				color_context.fillStyle = value;
				color_context.fillRect(0, 0, 1, 1);
				const pixel = color_context.getImageData(0, 0, 1, 1).data;
				return [pixel[0] / 255, pixel[1] / 255, pixel[2] / 255] as const;
			});
			color_cache.set(token, color);
			return color;
			});

		const SetColor = (name: string, token: SurfaceToken, alpha: number) =>
			Effect.gen(function* () {
			const [red, green, blue] = yield* ResolveSurface(token);
			yield* RunBrowserDom(() => gl.uniform4f(gl.getUniformLocation(program, name), red, green, blue, alpha));
			});

		let current_config: ShaderConfig;
		const renders = yield* Queue.unbounded<
			| { readonly _tag: "config"; readonly config: ShaderConfig }
			| { readonly _tag: "render"; readonly now: number }
		>();
		const ApplyConfig = (config: ShaderConfig) =>
			Effect.gen(function* () {
			current_config = config;
			yield* RunBrowserDom(() => {
				gl.uniform1f(gl.getUniformLocation(program, "u_bloom"), config.bloom);
				gl.uniform1f(gl.getUniformLocation(program, "u_intensity"), config.intensity);
				gl.uniform1f(gl.getUniformLocation(program, "u_density"), config.density);
				gl.uniform1f(gl.getUniformLocation(program, "u_spotty"), config.spotty);
				gl.uniform1f(gl.getUniformLocation(program, "u_midSize"), config.mid_size);
				gl.uniform1f(gl.getUniformLocation(program, "u_midIntensity"), config.mid_intensity);
				gl.uniform1f(gl.getUniformLocation(program, "u_scale"), config.scale);
				gl.uniform1f(gl.getUniformLocation(program, "u_rotation"), config.rotation);
				gl.uniform1f(gl.getUniformLocation(program, "u_offsetX"), config.offset_x);
				gl.uniform1f(gl.getUniformLocation(program, "u_offsetY"), config.offset_y);
			});
			yield* SetColor("u_color1", config.color_1, 0.64);
			yield* SetColor("u_color2", config.color_2, 0.78);
			yield* SetColor("u_color3", config.color_3, 0.92);
			yield* SetColor("u_color4", config.color_4, 0.82);
			yield* SetColor("u_color5", config.color_5, 0.72);
			yield* SetColor("u_colorBack", config.color_back, config.back_opacity);
			yield* SetColor("u_colorBloom", config.color_bloom, 0.72);
			yield* RunBrowserDom(() => Queue.offerUnsafe(renders, { _tag: "render", now: performance.now() }));
			});
		const unsubscribe = shader_config.subscribe((config) =>
			Queue.offerUnsafe(renders, { _tag: "config", config }),
		);
		releases.push(unsubscribe);

		let frame_id = 0;
		const reduced_motion = yield* RunBrowserDom(() => matchMedia("(prefers-reduced-motion: reduce)").matches);
		const Render = (now: number) =>
			Effect.gen(function* () {
			yield* RunBrowserDom(() => {
				const bounds = canvas.getBoundingClientRect();
				const pixel_ratio = Math.min(devicePixelRatio, 1.25);
				const width = Math.max(1, Math.round(bounds.width * pixel_ratio));
				const height = Math.max(1, Math.round(bounds.height * pixel_ratio));
				if (canvas.width !== width || canvas.height !== height) {
					canvas.width = width;
					canvas.height = height;
					gl.viewport(0, 0, width, height);
				}
				gl.uniform2f(resolution, width, height);
				gl.uniform1f(time, reduced_motion ? 1.8 : now * 0.00012 * current_config.speed);
				gl.clearColor(0, 0, 0, 0);
				gl.clear(gl.COLOR_BUFFER_BIT);
				gl.drawArrays(gl.TRIANGLES, 0, 6);
				if (!reduced_motion) frame_id = requestAnimationFrame((next) => Queue.offerUnsafe(renders, { _tag: "render", now: next }));
			});
			});
		yield* Effect.gen(function* () {
			while (true) {
				const command = yield* Queue.take(renders);
				if (command._tag === "config") yield* ApplyConfig(command.config);
				else yield* Render(command.now);
			}
		}).pipe(Effect.forkScoped);

		frame_id = yield* RunBrowserDom(() => requestAnimationFrame((now) => Queue.offerUnsafe(renders, { _tag: "render", now })));
		releases.push(
			Effect.gen(function* () {
				yield* RunBrowserDom(() => cancelAnimationFrame(frame_id));
			}),
		);
		return ReleaseResources;
		});

	const RunPaperRays = (canvas: HTMLCanvasElement) =>
		Effect.gen(function* () {
			yield* Effect.acquireRelease(
				AcquirePaperRays(canvas),
				(release) =>
					Effect.gen(function* () {
						if (release !== undefined) yield* release;
					}),
			);
			yield* Effect.never;
		});

	const paper_rays_runner = yield* MakeScopedAttachmentRunner(RunPaperRays);

	const PaperRays = (canvas: HTMLCanvasElement) => paper_rays_runner.Attachment(canvas);
</script>

<canvas aria-hidden="true" class="size-full" {@attach PaperRays}></canvas>
