# Ganbot Model Gallery

## Overview

The Ganbot Model Gallery is an interactive web interface for showcasing and comparing AI image generation models. It displays curated collections of models with generated samples, allowing users to browse different models, apply various artistic styles, and examine detailed technical specifications for each model.

**Location:** https://ganbot.brage.info/gallery/models

## Gallery Interface

### Layout Structure

The gallery uses a clean two-column layout:

- **Left Sidebar**: Navigation menu with dark blue/teal background containing:
  - "Ganbot" header branding
  - "Home" link
  - "Model Gallery" link (current page)
  - "Help" link

- **Main Content Area**: Light gray background with:
  - Page heading ("Model Gallery")
  - Filter controls
  - Model gallery table
  - Pagination controls

### Gallery Display

Models are organized in a table format with each row representing a single model:

- **Left Column**: Model name with optional classification tags (displayed as gray badges)
  - Example tags: "fanart", "fanart-cute", "dream", "qwen"

- **Right Column**: Four sample images (2x2 grid) generated with the same prompt
  - Displayed as WebP thumbnails at 200×75 resolution
  - All images use identical generation parameters for fair comparison
  - Standard test prompt: "1girl, portrait, outdoors, scenery, mountainous horizon"

## Filtering System

### Tag Filters (Top Row)

These filters categorize models by their intended use or training dataset:

- **All** - Show all available models
- **Recommended** - Curated selection of high-quality models (default)
- **Booru** - Models trained on booru-style datasets
- **English** - Models trained primarily on English text data
- **Fanart** - Models optimized for anime/fanart generation

**Behavior:**
- Click to apply filter (active filter highlighted in blue)
- Multiple tag filters can be combined with style filters
- URL updates with `?tag=` parameter

### Style Filters (Second Row)

These filters modify the generation prompt to showcase different artistic styles:

- **Default** - Standard photorealistic style (default)
- **Brushwork** - Adds painterly brush texture to prompt
- **Impressionist** - Applies impressionist art style modifiers
- **Ink Sketch** - Renders as ink sketch/line art
- **Pixel Art** - Generates retro pixel art style (prompt prefix: "pixel art, 8bit, retro game style")
- **Watercolor** - Applies watercolor painting aesthetic

**Behavior:**
- Click to apply style filter (active filter highlighted in blue)
- Style modifiers are prepended to the base prompt
- Each combination of tag and style filter creates a unique gallery view
- URL updates with `?style=` parameter

### Combining Filters

- Tag and style filters work independently and can be combined
- Example: "Fanart" tag + "Pixel Art" style shows all fanart models rendered in pixel art
- Filtering is instantaneous with no loading delay
- URL reflects all active filters: `?tag=fanart&style=pixel+art`

## Pagination

### Navigation Controls

Pagination controls appear both above and below the model gallery:

- **Previous Button** (`‹ Prev`) - Disabled when on first page
- **Page Numbers** - Shows 9 numbered buttons (1-9), with current page highlighted in blue
- **Next Button** (`Next ›`) - Disabled when on last page

### How Pagination Works

- Each page displays 5-13 models depending on active filters
- **Important**: Advancing pages doesn't show new models, but rather shows the same models with *different test prompts*
- This allows users to see how each model handles various prompt variations
- Current pagination state is tracked via `?col=` URL parameter
- Example prompts across pages:
  - Page 1: "1girl, portrait, outdoors, scenery, mountainous horizon"
  - Page 2: "1girl, 1boy, hinamori amu, sitting..."
  - (And so on through different variations)

## Detailed Image Modal

### Opening the Modal

Click any thumbnail image to open the detailed modal view. The modal overlays the gallery with a dark semi-transparent background.

### Modal Layout

The modal is divided into three main sections:

#### Left Panel: Technical Information

**Model Header:**
- Model name in large cyan text
- Brief model description

**Model Configuration:**
- **Checkpoint**: Full filesystem path to the model file
  - Example: `chroma/Chroma1-HD.safetensors`
- **Resolution**: Output image dimensions
  - Example: `1024x1024`

**Sampling Parameters:**
- **Steps**: Number of denoising steps (typically 20-40)
  - Higher values = more refined results, slower generation
- **CFG**: Classifier-Free Guidance scale (e.g., 4)
  - Controls how strongly the model follows the prompt
- **Sampler**: Algorithm used for diffusion
  - Common types: `euler_ancestral`, `dpmpp_2m`, `heun`
- **Scheduler**: Noise scheduling strategy
  - Example: `sgm_uniform`, `karras`

**Two-Stage Upscaling:**
- Shows "Enabled" or "Disabled"
- Indicates if the output uses post-generation upscaling

**Full Prompt:**
- Complete text used to generate all displayed images

#### Center/Right Area: Image Grid

- Four generated images displayed in 2×2 grid
- High-quality preview showing full generation result
- Images maintain proper aspect ratio and visual clarity

#### Navigation Controls

Arrow buttons allow navigation through the gallery:

- **↑ (Up Arrow)** - Navigate to previous model
- **← (Left Arrow)** - Navigate to previous prompt variation
- **→ (Right Arrow)** - Navigate to next prompt variation
- **↓ (Down Arrow)** - Navigate to next model
- **⊖ (Zoom Out)** - Reduce image size (disabled at minimum zoom)
- **⊕ (Zoom In)** - Enlarge images for detail examination
- **× (Close)** - Close modal and return to gallery

**Navigation Behavior:**
- Arrow buttons are disabled (grayed out) when at boundaries
  - Left/right arrows disabled on first/last prompt variation
  - Up/down arrows disabled at first/last model
- Active button highlighted with blue outline
- Navigation updates URL with modal state: `?modal=true&model=chroma&prompt=...`

### Modal Interaction

Users can:
- Navigate between models vertically (up/down arrows)
- Navigate between prompt variations horizontally (left/right arrows)
- Zoom in/out to examine details or get full view
- Close modal to return to main gallery
- Copy text (including prompts) for their own use

## Technical Details

### Image Specifications

**Format and Compression:**
- Format: WebP with lossy compression for efficiency
- Thumbnail size: 200×75 pixels
- URL structure: `/image/200/75/{UUID}.webp`
- Pre-generated and cached for instant loading

**Image Metadata:**
Each image includes:
- Model name and version
- Complete generation prompt
- Full technical parameters (steps, CFG, sampler, scheduler)
- Model tags and categories
- Generation timestamp

### URL Structure

**Base Path:** `/gallery/models`

**Query Parameters:**
- `tag` - Active tag filter: `recommended`, `fanart`, `booru`, `english`, `all`
- `style` - Active style filter: `default`, `brushwork`, `impressionist`, `ink sketch`, `pixel art`, `watercolor`
- `col` - Pagination page number (column index)
- `modal` - Boolean flag indicating if modal is open
- `model` - Current model displayed in modal (when modal is open)
- `prompt` - Current prompt index (when modal is open)

**Example URL:**
```
/gallery/models?tag=fanart&style=pixel+art&col=2&modal=true&model=chroma&prompt=1
```

### Performance

- **Instant filtering**: No loading delays when applying filters
- **Fast pagination**: Page transitions are immediate
- **Pre-generated images**: All samples are pre-rendered and cached
- **No lazy loading**: Visible images load immediately
- **Smooth modal**: Modal open/close operations are fluid

## Use Cases

### For Model Selection
Users can compare how different models perform on identical prompts, helping them choose the right model for their specific use case.

### For Style Exploration
Style filters show how different models interpret various artistic aesthetics, from photorealistic to pixel art.

### For Technical Learning
The detailed modal view provides complete generation parameters, helping users understand what settings produce quality results.

### For Prompt Engineering
Users can examine successful prompts used for generation and adapt them for their own image generation workflows.

## Tips for Using the Gallery

1. **Start with Recommended Models** - The default "Recommended" tag shows hand-picked quality models
2. **Test Different Styles** - Switch between style filters to see how your favorite model handles different aesthetics
3. **Compare Within a Style** - Page through models with a specific style active to compare similar outputs
4. **Examine Technical Details** - Open the modal to understand the exact parameters used for good results
5. **Use Pagination Wisely** - Pages show different prompts, so cycle through to see prompt robustness
6. **Zoom for Details** - Use zoom controls in the modal to inspect fine details and quality

## Gallery Statistics

The gallery typically contains:
- 50+ curated AI image generation models
- 100+ unique prompt variations per model
- 400+ pages of gallery content (models × prompts)
- Thousands of pre-generated sample images
- Multiple artistic style variations

## Future Enhancements

Potential features for gallery expansion:
- User ratings and reviews for models
- Custom prompt testing tool
- Model benchmark comparisons
- Community contributed models
- Advanced filtering by model type (SDXL, Flux, etc.)
- Side-by-side comparison mode
- Export of prompts and settings
