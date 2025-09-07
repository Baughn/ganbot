//! Parser for raw (textual) image generation prompts.
//! Converts a prompt like "a girl on the beach -m flux --no beachball" into a Generate struct.
//!
//! This acts mostly like a regular command-line parser, with the exception that `--no` is modal.
//! Every non-option word after --no will be added to negative_prompt instead of prompt.
//!
//! Otherwise, we support:
//! -c, --count for num_images
//! -w, --width, -h, --height and --ar for image size settings.
//! -m, --model for model choice
//! --seed
//! -s, --steps

use crate::messages::imagen::{Generate, References};
use anyhow::{Result, anyhow, bail};

impl Generate {
    pub fn from_str(raw: &str) -> Result<Generate> {
        let mut prompt_parts = Vec::new();
        let mut negative_parts = Vec::new();
        let mut in_negative_mode = false;

        let mut num_images = None;
        let mut width = None;
        let mut height = None;
        let mut aspect = None;
        let mut model = None;
        let mut seed = None;
        let mut steps = None;

        let tokens: Vec<&str> = raw.split_whitespace().collect();
        let mut i = 0;

        while i < tokens.len() {
            let token = tokens[i];

            match token {
                "--no" => {
                    in_negative_mode = true;
                    i += 1;
                }
                "-c" | "--count" => {
                    i += 1;
                    if i >= tokens.len() {
                        bail!("Option {} requires a value", token);
                    }
                    num_images = Some(parse_u32(tokens[i], "num_images")?);
                    i += 1;
                }
                "-w" | "--width" => {
                    i += 1;
                    if i >= tokens.len() {
                        bail!("Option {} requires a value", token);
                    }
                    width = Some(parse_u32(tokens[i], "width")?);
                    i += 1;
                }
                "-h" | "--height" => {
                    i += 1;
                    if i >= tokens.len() {
                        bail!("Option {} requires a value", token);
                    }
                    height = Some(parse_u32(tokens[i], "height")?);
                    i += 1;
                }
                "--ar" | "--aspect" => {
                    i += 1;
                    if i >= tokens.len() {
                        bail!("Option {} requires a value", token);
                    }
                    aspect = Some(parse_aspect_ratio(tokens[i])?);
                    i += 1;
                }
                "-m" | "--model" => {
                    i += 1;
                    if i >= tokens.len() {
                        bail!("Option {} requires a value", token);
                    }
                    model = Some(tokens[i].to_string());
                    i += 1;
                }
                "--seed" => {
                    i += 1;
                    if i >= tokens.len() {
                        bail!("Option {} requires a value", token);
                    }
                    seed = Some(parse_u64(tokens[i], "seed")?);
                    i += 1;
                }
                "-s" | "--steps" => {
                    i += 1;
                    if i >= tokens.len() {
                        bail!("Option {} requires a value", token);
                    }
                    steps = Some(parse_u32(tokens[i], "steps")?);
                    i += 1;
                }
                _ if token.starts_with('-') && !token.starts_with("--") && token.len() > 2 => {
                    // Handle combined short options like -w512
                    // Only try to parse if it looks like a valid combined option (number after flag)
                    let flag = &token[0..2];
                    let value = &token[2..];

                    // Check if value starts with a digit for numeric options
                    let looks_like_combined = match flag {
                        "-c" | "-w" | "-h" | "-s" => {
                            value.chars().next().is_some_and(|c| c.is_ascii_digit())
                        }
                        "-m" => true, // Model names don't need to start with a digit
                        _ => false,
                    };

                    if looks_like_combined {
                        match flag {
                            "-c" => num_images = Some(parse_u32(value, "num_images")?),
                            "-w" => width = Some(parse_u32(value, "width")?),
                            "-h" => height = Some(parse_u32(value, "height")?),
                            "-m" => model = Some(value.to_string()),
                            "-s" => steps = Some(parse_u32(value, "steps")?),
                            _ => {
                                // Unknown option, treat as part of prompt
                                if in_negative_mode {
                                    negative_parts.push(token);
                                } else {
                                    prompt_parts.push(token);
                                }
                            }
                        }
                    } else {
                        // Not a valid combined option, treat as part of prompt
                        if in_negative_mode {
                            negative_parts.push(token);
                        } else {
                            prompt_parts.push(token);
                        }
                    }
                    i += 1;
                }
                _ if token.starts_with("--") && token.contains('=') => {
                    // Handle long options with = like --width=512
                    let parts: Vec<&str> = token.splitn(2, '=').collect();
                    if parts.len() != 2 {
                        if in_negative_mode {
                            negative_parts.push(token);
                        } else {
                            prompt_parts.push(token);
                        }
                        i += 1;
                        continue;
                    }
                    let option = parts[0];
                    let value = parts[1];

                    match option {
                        "--count" => num_images = Some(parse_u32(value, "num_images")?),
                        "--width" => width = Some(parse_u32(value, "width")?),
                        "--height" => height = Some(parse_u32(value, "height")?),
                        "--aspect" | "--ar" => aspect = Some(parse_aspect_ratio(value)?),
                        "--model" => model = Some(value.to_string()),
                        "--seed" => seed = Some(parse_u64(value, "seed")?),
                        "--steps" => steps = Some(parse_u32(value, "steps")?),
                        _ => {
                            // Unknown option, treat as part of prompt
                            if in_negative_mode {
                                negative_parts.push(token);
                            } else {
                                prompt_parts.push(token);
                            }
                        }
                    }
                    i += 1;
                }
                _ => {
                    // Regular word - add to prompt or negative prompt
                    if in_negative_mode {
                        negative_parts.push(token);
                    } else {
                        prompt_parts.push(token);
                    }
                    i += 1;
                }
            }
        }

        let prompt = prompt_parts.join(" ");
        let negative_prompt = if negative_parts.is_empty() {
            None
        } else {
            Some(negative_parts.join(" "))
        };

        // Validate that we have at least a prompt or negative prompt
        if prompt.is_empty() && negative_prompt.is_none() {
            bail!("No prompt provided");
        }

        Ok(Generate {
            raw_prompt: raw.to_string(),
            prompt,
            negative_prompt,
            num_images,
            aspect,
            width,
            height,
            model,
            seed,
            steps,
            references: References {
                img2img: None,
                img2img_strength: None,
                context: Vec::new(),
            },
        })
    }
}

fn parse_u32(s: &str, field_name: &str) -> Result<u32> {
    s.parse::<u32>()
        .map_err(|_| anyhow!("Invalid {} value: {}", field_name, s))
}

fn parse_u64(s: &str, field_name: &str) -> Result<u64> {
    s.parse::<u64>()
        .map_err(|_| anyhow!("Invalid {} value: {}", field_name, s))
}

fn parse_aspect_ratio(s: &str) -> Result<(u32, u32)> {
    // Support formats like "16:9", "16x9", "16/9"
    let separators = [':', 'x', 'X', '/', '-'];

    for sep in separators {
        if s.contains(sep) {
            let parts: Vec<&str> = s.split(sep).collect();
            if parts.len() == 2 {
                let width = parts[0]
                    .parse::<u32>()
                    .map_err(|_| anyhow!("Invalid aspect ratio width: {}", parts[0]))?;
                let height = parts[1]
                    .parse::<u32>()
                    .map_err(|_| anyhow!("Invalid aspect ratio height: {}", parts[1]))?;

                if width == 0 || height == 0 {
                    bail!("Aspect ratio dimensions must be non-zero");
                }

                return Ok((width, height));
            }
        }
    }

    bail!(
        "Invalid aspect ratio format: {}. Use formats like 16:9, 16x9, or 16/9",
        s
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_prompt() {
        let result = Generate::from_str("a beautiful sunset over the ocean").unwrap();
        assert_eq!(result.prompt, "a beautiful sunset over the ocean");
        assert_eq!(result.negative_prompt, None);
        assert_eq!(result.model, None);
        assert_eq!(result.width, None);
        assert_eq!(result.height, None);
    }

    #[test]
    fn test_prompt_with_model() {
        let result = Generate::from_str("a cat -m flux").unwrap();
        assert_eq!(result.prompt, "a cat");
        assert_eq!(result.model, Some("flux".to_string()));

        let result2 = Generate::from_str("a dog --model dalle3").unwrap();
        assert_eq!(result2.prompt, "a dog");
        assert_eq!(result2.model, Some("dalle3".to_string()));
    }

    #[test]
    fn test_prompt_with_dimensions() {
        let result = Generate::from_str("landscape -w 1024 -h 768").unwrap();
        assert_eq!(result.prompt, "landscape");
        assert_eq!(result.width, Some(1024));
        assert_eq!(result.height, Some(768));

        let result2 = Generate::from_str("portrait --width 512 --height 1024").unwrap();
        assert_eq!(result2.prompt, "portrait");
        assert_eq!(result2.width, Some(512));
        assert_eq!(result2.height, Some(1024));
    }

    #[test]
    fn test_prompt_with_aspect_ratio() {
        let result = Generate::from_str("wide image --ar 16:9").unwrap();
        assert_eq!(result.prompt, "wide image");
        assert_eq!(result.aspect, Some((16, 9)));

        let result2 = Generate::from_str("square --aspect 1x1").unwrap();
        assert_eq!(result2.prompt, "square");
        assert_eq!(result2.aspect, Some((1, 1)));

        let result3 = Generate::from_str("tall --ar 9/16").unwrap();
        assert_eq!(result3.prompt, "tall");
        assert_eq!(result3.aspect, Some((9, 16)));
    }

    #[test]
    fn test_prompt_with_negative() {
        let result = Generate::from_str("beach scene --no umbrella chairs people").unwrap();
        assert_eq!(result.prompt, "beach scene");
        assert_eq!(
            result.negative_prompt,
            Some("umbrella chairs people".to_string())
        );
    }

    #[test]
    fn test_prompt_with_multiple_options() {
        let result =
            Generate::from_str("cyberpunk city -m flux -w 1024 -h 768 --seed 42 -s 30 --no blur")
                .unwrap();
        assert_eq!(result.prompt, "cyberpunk city");
        assert_eq!(result.model, Some("flux".to_string()));
        assert_eq!(result.width, Some(1024));
        assert_eq!(result.height, Some(768));
        assert_eq!(result.seed, Some(42));
        assert_eq!(result.steps, Some(30));
        assert_eq!(result.negative_prompt, Some("blur".to_string()));
    }

    #[test]
    fn test_combined_short_options() {
        let result = Generate::from_str("test -w512 -h256").unwrap();
        assert_eq!(result.prompt, "test");
        assert_eq!(result.width, Some(512));
        assert_eq!(result.height, Some(256));
    }

    #[test]
    fn test_long_options_with_equals() {
        let result = Generate::from_str("test --width=1920 --height=1080 --model=sdxl").unwrap();
        assert_eq!(result.prompt, "test");
        assert_eq!(result.width, Some(1920));
        assert_eq!(result.height, Some(1080));
        assert_eq!(result.model, Some("sdxl".to_string()));
    }

    #[test]
    fn test_count_option() {
        let result = Generate::from_str("multiple cats -c 4").unwrap();
        assert_eq!(result.prompt, "multiple cats");
        assert_eq!(result.num_images, Some(4));

        let result2 = Generate::from_str("batch --count 10").unwrap();
        assert_eq!(result2.prompt, "batch");
        assert_eq!(result2.num_images, Some(10));
    }

    #[test]
    fn test_empty_prompt_with_negative() {
        // Empty prompt with only negative is allowed
        let result = Generate::from_str("--no blurry distorted").unwrap();
        assert_eq!(result.prompt, "");
        assert_eq!(result.negative_prompt, Some("blurry distorted".to_string()));
    }

    #[test]
    fn test_empty_input() {
        let result = Generate::from_str("");
        assert!(result.is_err());

        let result2 = Generate::from_str("   ");
        assert!(result2.is_err());
    }

    #[test]
    fn test_invalid_numeric_values() {
        let result = Generate::from_str("test -w abc");
        assert!(result.is_err());

        let result2 = Generate::from_str("test --seed notanumber");
        assert!(result2.is_err());

        let result3 = Generate::from_str("test -s -10");
        assert!(result3.is_err());
    }

    #[test]
    fn test_invalid_aspect_ratio() {
        let result = Generate::from_str("test --ar 16");
        assert!(result.is_err());

        let result2 = Generate::from_str("test --ar 0:9");
        assert!(result2.is_err());

        let result3 = Generate::from_str("test --ar abc:def");
        assert!(result3.is_err());
    }

    #[test]
    fn test_missing_option_values() {
        let result = Generate::from_str("test -w");
        assert!(result.is_err());

        let result2 = Generate::from_str("test --model");
        assert!(result2.is_err());
    }

    #[test]
    fn test_unknown_options_treated_as_prompt() {
        let result = Generate::from_str("a --fantasy scene with --magic").unwrap();
        assert_eq!(result.prompt, "a --fantasy scene with --magic");

        let result2 = Generate::from_str("test -xyz something").unwrap();
        assert_eq!(result2.prompt, "test -xyz something");
    }

    #[test]
    fn test_negative_mode_with_unknown_options() {
        // Unknown options after --no go to negative prompt
        let result = Generate::from_str("scene --no --weird -stuff").unwrap();
        assert_eq!(result.prompt, "scene");
        assert_eq!(result.negative_prompt, Some("--weird -stuff".to_string()));
    }

    #[test]
    fn test_complex_real_world_example() {
        let input = "a majestic dragon flying over mountains -m flux --ar 16:9 -s 50 --seed 12345 --no blur text watermark";
        let result = Generate::from_str(input).unwrap();

        assert_eq!(result.raw_prompt, input);
        assert_eq!(result.prompt, "a majestic dragon flying over mountains");
        assert_eq!(result.model, Some("flux".to_string()));
        assert_eq!(result.aspect, Some((16, 9)));
        assert_eq!(result.steps, Some(50));
        assert_eq!(result.seed, Some(12345));
        assert_eq!(
            result.negative_prompt,
            Some("blur text watermark".to_string())
        );
    }

    #[test]
    fn test_options_after_negative_mode() {
        // Options after --no should still be parsed as options
        let result = Generate::from_str("test --no bad stuff -w 100").unwrap();
        assert_eq!(result.prompt, "test");
        assert_eq!(result.negative_prompt, Some("bad stuff".to_string()));
        assert_eq!(result.width, Some(100));
    }
}
