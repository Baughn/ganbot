# Ganbot Commands

Ganbot is a multi-platform bot with AI-powered image generation capabilities.

## Image Generation Commands

### !prompt - Direct Image Generation

Generate images with precise control over model and parameters.

**Basic Syntax:**
```
!prompt <description> [options]
```

**Available Options:**

- `-m <model>` or `--model <model>` - Select the model to use (e.g., `flux`, `sdxl`)
- `-c <number>` or `--count <number>` - Number of images to generate (default: 1)
- `-w <pixels>` or `--width <pixels>` - Image width
- `-h <pixels>` or `--height <pixels>` - Image height
- `--ar <ratio>` or `--aspect <ratio>` - Aspect ratio (e.g., `16:9`, `1:1`, `9:16`)
- `--seed <number>` - Random seed for reproducibility
- `-s <number>` or `--steps <number>` - Number of inference steps
- `--denoise <0.0-1.0>` - Denoising strength for img2img
- `--alias <name>` - Load settings from a saved alias
- `--no <terms>` - Negative prompt (everything after `--no` is excluded from the image)

**Option Shortcuts:**

You can combine short options with their values:
- `-w512` instead of `-w 512`
- `-h768` instead of `-h 768`
- `-mflux` instead of `-m flux`

You can also use equals signs with long options:
- `--width=1920` instead of `--width 1920`
- `--model=sdxl` instead of `--model sdxl`

**Examples:**

Simple generation:
```
!prompt a majestic dragon flying over snow-capped mountains
```

Specify a model:
```
!prompt a cyberpunk city at night -m flux
```

Control dimensions with aspect ratio:
```
!prompt beautiful landscape --ar 16:9 -m sdxl
```

Control dimensions precisely:
```
!prompt portrait of a wizard -w 512 -h 768
```

Generate multiple images at once:
```
!prompt cute robot -c 4 -m flux
```

Use negative prompts to exclude elements:
```
!prompt beach scene --no umbrella people crowds
```

Full control with multiple options:
```
!prompt futuristic spaceship -m flux --ar 16:9 -s 50 --seed 42 --no blur text
```

Reproducible generation:
```
!prompt sunset over ocean --seed 12345 -m sdxl
```

### !dream - Artistic Variations

Generate multiple artistic variations of your concept with creative AI-enhanced prompts and detective-themed commentary.

**Basic Syntax:**
```
!dream <description> [options]
```

**How it Works:**

`!dream` takes your request and creates multiple unique variations by:
1. Generating distinct, detailed prompts for each variation
2. Creating images from those enhanced prompts
3. Presenting them in a gallery with creative commentary

**Key Differences from !prompt:**

- Defaults to 2 variations (controlled with `-c`, max 6)
- Uses the "dream" model alias by default
- Enhances your prompt with thematic details and atmosphere
- Includes creative commentary with results
- Only works with English-friendly models

**Supported Options:**

All !prompt options work, but the most useful are:
- `-m <model>` - Select a different model
- `-c <number>` - Number of variations (1-6, default: 2)
- `--ar <ratio>` - Aspect ratio
- `-w/-h` - Dimensions

**Examples:**

Basic dream request (generates 2 variations):
```
!dream a mysterious forest at twilight
```

More variations:
```
!dream steampunk airship -c 4
```

Different model and aspect ratio:
```
!dream neon-lit alleyway -m flux --ar 16:9
```

Landscape with specific dimensions:
```
!dream ancient ruins -c 3 -w 1024 -h 768
```

## Getting Help

For more information, visit the [Model Gallery](/gallery/models) to see examples of what different models can generate.
