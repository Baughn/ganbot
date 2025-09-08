// This is an API wrapper; not all code is expected to be used.
#![allow(dead_code)]

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeData {
    pub inputs: Map<String, Value>,
    pub class_type: String,
    #[serde(rename = "_meta")]
    pub meta: Option<NodeMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMeta {
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NodeOutput<T> {
    pub node_id: String,
    pub output_index: usize,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> NodeOutput<T> {
    fn new(node_id: String, output_index: usize) -> Self {
        Self {
            node_id,
            output_index,
            _phantom: std::marker::PhantomData,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Model;
#[derive(Debug, Clone)]
pub struct Clip;
#[derive(Debug, Clone)]
pub struct Vae;
#[derive(Debug, Clone)]
pub struct Latent;
#[derive(Debug, Clone)]
pub struct Conditioning;
#[derive(Debug, Clone)]
pub struct Image;

pub type ModelOutput = NodeOutput<Model>;
pub type ClipOutput = NodeOutput<Clip>;
pub type VaeOutput = NodeOutput<Vae>;
pub type LatentOutput = NodeOutput<Latent>;
pub type ConditioningOutput = NodeOutput<Conditioning>;
pub type ImageOutput = NodeOutput<Image>;

#[derive(Debug, Clone)]
pub struct KSamplerParams {
    pub seed: u64,
    pub steps: u32,
    pub cfg: f32,
    pub sampler: String,
    pub scheduler: String,
    pub denoise: f32,
}

impl Default for KSamplerParams {
    fn default() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        Self {
            seed,
            steps: 20,
            cfg: 7.0,
            sampler: "euler".to_string(),
            scheduler: "normal".to_string(),
            denoise: 1.0,
        }
    }
}

#[derive(Debug)]
pub struct Graph {
    nodes: IndexMap<String, NodeData>,
    node_counts: HashMap<String, usize>,
}

impl Graph {
    pub fn new() -> Self {
        Self {
            nodes: IndexMap::new(),
            node_counts: HashMap::new(),
        }
    }

    fn generate_node_id(&mut self, class_type: &str) -> String {
        let count = self
            .node_counts
            .entry(class_type.to_string())
            .and_modify(|c| *c += 1)
            .or_insert(1);

        if *count == 1 {
            class_type.to_string()
        } else {
            format!("{}-{}", class_type, *count)
        }
    }

    fn add_node(&mut self, class_type: &str, inputs: Map<String, Value>) -> String {
        let node_id = self.generate_node_id(class_type);
        let node = NodeData {
            inputs,
            class_type: class_type.to_string(),
            meta: Some(NodeMeta {
                title: Some(class_type.to_string()),
            }),
        };
        self.nodes.insert(node_id.clone(), node);
        node_id
    }

    fn node_reference<T>(output: &NodeOutput<T>) -> Value {
        Value::Array(vec![
            Value::String(output.node_id.clone()),
            Value::Number(serde_json::Number::from(output.output_index)),
        ])
    }

    pub fn checkpoint_loader(&mut self, ckpt_name: &str) -> (ModelOutput, ClipOutput, VaeOutput) {
        let mut inputs = Map::new();
        inputs.insert(
            "ckpt_name".to_string(),
            Value::String(ckpt_name.to_string()),
        );

        let node_id = self.add_node("CheckpointLoaderSimple", inputs);
        (
            NodeOutput::new(node_id.clone(), 0),
            NodeOutput::new(node_id.clone(), 1),
            NodeOutput::new(node_id, 2),
        )
    }

    pub fn unet_loader(&mut self, unet_name: &str) -> ModelOutput {
        let mut inputs = Map::new();
        inputs.insert(
            "unet_name".to_string(),
            Value::String(unet_name.to_string()),
        );
        inputs.insert(
            "weight_dtype".to_string(),
            Value::String("default".to_string()),
        );

        let node_id = self.add_node("UNETLoader", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn clip_loader(&mut self, clip_name: &str) -> ClipOutput {
        self.clip_loader_with_type(clip_name, "qwen_image")
    }

    pub fn clip_loader_with_type(&mut self, clip_name: &str, clip_type: &str) -> ClipOutput {
        let mut inputs = Map::new();
        inputs.insert(
            "clip_name".to_string(),
            Value::String(clip_name.to_string()),
        );
        inputs.insert("type".to_string(), Value::String(clip_type.to_string()));
        inputs.insert("device".to_string(), Value::String("default".to_string()));

        let node_id = self.add_node("CLIPLoader", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn dual_clip_loader(&mut self, clip_name1: &str, clip_name2: &str) -> ClipOutput {
        let mut inputs = Map::new();
        inputs.insert(
            "clip_name1".to_string(),
            Value::String(clip_name1.to_string()),
        );
        inputs.insert(
            "clip_name2".to_string(),
            Value::String(clip_name2.to_string()),
        );
        inputs.insert("type".to_string(), Value::String("flux".to_string()));
        inputs.insert("device".to_string(), Value::String("default".to_string()));

        let node_id = self.add_node("DualCLIPLoader", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn vae_loader(&mut self, vae_name: &str) -> VaeOutput {
        let mut inputs = Map::new();
        inputs.insert("vae_name".to_string(), Value::String(vae_name.to_string()));

        let node_id = self.add_node("VAELoader", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn clip_text_encode(&mut self, clip: &ClipOutput, text: &str) -> ConditioningOutput {
        let mut inputs = Map::new();
        inputs.insert("text".to_string(), Value::String(text.to_string()));
        inputs.insert("clip".to_string(), Self::node_reference(clip));

        let node_id = self.add_node("CLIPTextEncode", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn empty_latent_image(&mut self, width: u32, height: u32, batch_size: u32) -> LatentOutput {
        let mut inputs = Map::new();
        inputs.insert(
            "width".to_string(),
            Value::Number(serde_json::Number::from(width)),
        );
        inputs.insert(
            "height".to_string(),
            Value::Number(serde_json::Number::from(height)),
        );
        inputs.insert(
            "batch_size".to_string(),
            Value::Number(serde_json::Number::from(batch_size)),
        );

        let node_id = self.add_node("EmptyLatentImage", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn empty_sd3_latent_image(
        &mut self,
        width: u32,
        height: u32,
        batch_size: u32,
    ) -> LatentOutput {
        let mut inputs = Map::new();
        inputs.insert(
            "width".to_string(),
            Value::Number(serde_json::Number::from(width)),
        );
        inputs.insert(
            "height".to_string(),
            Value::Number(serde_json::Number::from(height)),
        );
        inputs.insert(
            "batch_size".to_string(),
            Value::Number(serde_json::Number::from(batch_size)),
        );

        let node_id = self.add_node("EmptySD3LatentImage", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn ksampler(
        &mut self,
        model: &ModelOutput,
        positive: &ConditioningOutput,
        negative: &ConditioningOutput,
        latent_image: &LatentOutput,
        params: KSamplerParams,
    ) -> LatentOutput {
        let mut inputs = Map::new();
        inputs.insert(
            "seed".to_string(),
            Value::Number(serde_json::Number::from(params.seed)),
        );
        inputs.insert(
            "steps".to_string(),
            Value::Number(serde_json::Number::from(params.steps)),
        );
        inputs.insert(
            "cfg".to_string(),
            serde_json::Number::from_f64(params.cfg as f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
        inputs.insert("sampler_name".to_string(), Value::String(params.sampler));
        inputs.insert("scheduler".to_string(), Value::String(params.scheduler));
        inputs.insert(
            "denoise".to_string(),
            serde_json::Number::from_f64(params.denoise as f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
        inputs.insert("model".to_string(), Self::node_reference(model));
        inputs.insert("positive".to_string(), Self::node_reference(positive));
        inputs.insert("negative".to_string(), Self::node_reference(negative));
        inputs.insert(
            "latent_image".to_string(),
            Self::node_reference(latent_image),
        );

        let node_id = self.add_node("KSampler", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn vae_decode(&mut self, vae: &VaeOutput, samples: &LatentOutput) -> ImageOutput {
        let mut inputs = Map::new();
        inputs.insert("samples".to_string(), Self::node_reference(samples));
        inputs.insert("vae".to_string(), Self::node_reference(vae));

        let node_id = self.add_node("VAEDecode", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn save_images(&mut self, images: &ImageOutput, filename_prefix: &str) -> ImageOutput {
        let mut inputs = Map::new();
        inputs.insert(
            "filename_prefix".to_string(),
            Value::String(filename_prefix.to_string()),
        );
        inputs.insert("images".to_string(), Self::node_reference(images));

        let node_id = self.add_node("SaveImage", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn websocket_save_image(&mut self, images: &ImageOutput) -> ImageOutput {
        let mut inputs = Map::new();
        inputs.insert("images".to_string(), Self::node_reference(images));

        let node_id = self.add_node("WebsocketSaveImage", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn conditioning_zero_out(
        &mut self,
        conditioning: &ConditioningOutput,
    ) -> ConditioningOutput {
        let mut inputs = Map::new();
        inputs.insert(
            "conditioning".to_string(),
            Self::node_reference(conditioning),
        );

        let node_id = self.add_node("ConditioningZeroOut", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn torch_compile_model(&mut self, model: &ModelOutput, backend: &str) -> ModelOutput {
        let mut inputs = Map::new();
        inputs.insert("model".to_string(), Self::node_reference(model));
        inputs.insert("backend".to_string(), Value::String(backend.to_string()));

        let node_id = self.add_node("TorchCompileModel", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn latent_upscaler(
        &mut self,
        latent: &LatentOutput,
        latent_ver: &str,
        scale_factor: f32,
    ) -> LatentOutput {
        let mut inputs = Map::new();
        inputs.insert("latent".to_string(), Self::node_reference(latent));
        inputs.insert("version".to_string(), Value::String(latent_ver.to_string()));
        inputs.insert(
            "upscale".to_string(),
            serde_json::Number::from_f64(scale_factor as f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );

        let node_id = self.add_node("NNLatentUpscale", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn model_sampling_aura_flow(&mut self, model: &ModelOutput, shift: f64) -> ModelOutput {
        let mut inputs = Map::new();
        inputs.insert(
            "shift".to_string(),
            serde_json::Number::from_f64(shift)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
        inputs.insert("model".to_string(), Self::node_reference(model));

        let node_id = self.add_node("ModelSamplingAuraFlow", inputs);
        NodeOutput::new(node_id, 0)
    }

    pub fn build(self) -> Value {
        let mut result = Map::new();
        for (id, node) in self.nodes {
            result.insert(id, serde_json::to_value(node).unwrap());
        }
        Value::Object(result)
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_txt2img_workflow() {
        let mut g = Graph::new();

        let (model, clip, vae) =
            g.checkpoint_loader("xl-ill/Hassaku%20XL%20(Illustrious)_2.2.safetensors");
        let positive = g.clip_text_encode(&clip, "hinamori amu, shugo chara, outdoors,");
        let negative =
            g.clip_text_encode(&clip, "text, watermark, low quality, worst quality, 3d,");
        let latent = g.empty_latent_image(1280, 800, 1);

        let params = KSamplerParams {
            seed: 310307348447692,
            steps: 20,
            cfg: 5.5,
            sampler: "er_sde".to_string(),
            scheduler: "beta".to_string(),
            denoise: 1.0,
        };
        let samples = g.ksampler(&model, &positive, &negative, &latent, params);
        let images = g.vae_decode(&vae, &samples);
        let _output = g.save_images(&images, "ComfyUI");

        let json = g.build();
        println!("{}", serde_json::to_string_pretty(&json).unwrap());

        let json_obj = json.as_object().unwrap();
        assert!(json_obj.contains_key("CheckpointLoaderSimple"));
        assert!(json_obj.contains_key("CLIPTextEncode"));
        assert!(json_obj.contains_key("CLIPTextEncode-2"));
        assert!(json_obj.contains_key("EmptyLatentImage"));
        assert!(json_obj.contains_key("KSampler"));
        assert!(json_obj.contains_key("VAEDecode"));
        assert!(json_obj.contains_key("SaveImage"));
    }

    #[test]
    fn test_qwen_workflow() {
        let mut g = Graph::new();

        let model = g.unet_loader("qwen_image_fp8_e4m3fn.safetensors");
        let clip = g.clip_loader("qwen_2.5_vl_7b_fp8_scaled.safetensors");
        let vae = g.vae_loader("qwen_image_vae.safetensors");

        let model_with_sampling = g.model_sampling_aura_flow(&model, 3.1000000000000005);

        let positive = g.clip_text_encode(&clip, "Brushwork. thigh-up framing. A twelve year old redhead is sleeping next to a lake in rural Finland. It's mid-summer, and there is a purple sun in the sky. She's lying on her back. She looks relaxed.");
        let negative = g.clip_text_encode(&clip, "");
        let latent = g.empty_sd3_latent_image(1328, 1024, 1);

        let params = KSamplerParams {
            seed: 153903009837362,
            steps: 20,
            cfg: 2.5,
            sampler: "er_sde".to_string(),
            scheduler: "sgm_uniform".to_string(),
            denoise: 1.0,
        };
        let samples = g.ksampler(&model_with_sampling, &positive, &negative, &latent, params);
        let images = g.vae_decode(&vae, &samples);
        let _output = g.save_images(&images, "ComfyUI");

        let json = g.build();
        let json_obj = json.as_object().unwrap();
        assert!(json_obj.contains_key("UNETLoader"));
        assert!(json_obj.contains_key("CLIPLoader"));
        assert!(json_obj.contains_key("VAELoader"));
        assert!(json_obj.contains_key("ModelSamplingAuraFlow"));
        assert!(json_obj.contains_key("EmptySD3LatentImage"));
    }

    #[test]
    fn test_flux_workflow() {
        let mut g = Graph::new();

        let model = g.unet_loader("flux1-krea-dev_fp8_scaled.safetensors");
        let clip = g.dual_clip_loader("clip_l.safetensors", "t5xxl_fp16.safetensors");
        let vae = g.vae_loader("ae.safetensors");

        let positive = g.clip_text_encode(&clip, "hinamori amu, shugo chara!");
        let negative = g.conditioning_zero_out(&positive);
        let latent = g.empty_sd3_latent_image(1280, 768, 1);

        let params = KSamplerParams {
            seed: 1011350887342639,
            steps: 28,
            cfg: 1.0,
            sampler: "er_sde".to_string(),
            scheduler: "beta".to_string(),
            denoise: 1.0,
        };
        let samples = g.ksampler(&model, &positive, &negative, &latent, params);
        let images = g.vae_decode(&vae, &samples);
        let _output = g.save_images(&images, "flux_krea/flux_krea");

        let json = g.build();
        let json_obj = json.as_object().unwrap();
        assert!(json_obj.contains_key("DualCLIPLoader"));
        assert!(json_obj.contains_key("ConditioningZeroOut"));
    }
}
