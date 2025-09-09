use anyhow::{Error, Result, bail};
use kameo::{Actor, prelude::Message};
use tracing::{debug, info};
use url::Url;

use crate::persistence::{
    images,
    user::{GetUserId, UserActor},
};
use crate::supervisor::Supervisor;

/// Actor for the !delete command - deletes images that the user has generated
#[derive(Actor)]
pub(crate) struct DeleteActor {
    user_actor: kameo::actor::ActorRef<UserActor>,
}

#[derive(Debug)]
pub struct DeleteResult {
    pub message: String,
}

impl DeleteActor {
    pub async fn new(user_actor: kameo::actor::ActorRef<UserActor>) -> Self {
        Self { user_actor }
    }
}

/// Delete an image, by either UUID or URL.
impl Message<String> for DeleteActor {
    type Reply = Result<DeleteResult, Error>;

    async fn handle(
        &mut self,
        msg: String,
        _ctx: &mut kameo::prelude::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("DeleteActor received input: {}", msg);

        let input = msg.trim();
        if input.is_empty() {
            bail!("No UUID or URL provided");
        }

        // Get the expected base URL from config for host validation
        let image_host_config = Supervisor::image_host().await;
        let expected_base_url = &image_host_config.base_url;

        // Extract UUID from input (either direct UUID or from URL)
        let uuid = extract_uuid_from_input(input, expected_base_url)?;
        debug!("Extracted UUID: {}", uuid);

        // Get the user ID to verify ownership
        let user_id = self
            .user_actor
            .ask(GetUserId)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get user id: {e:#}"))?;

        // Get the username for ownership verification
        let username = user_id.key();

        // Delete the image with ownership verification
        match images::delete_image(&uuid, &username).await {
            Ok(delete_result) => Ok(DeleteResult {
                message: delete_result.message,
            }),
            Err(e) => {
                info!(
                    "Delete failed for user {} and UUID {}: {}",
                    username, uuid, e
                );
                bail!("Delete failed: {}", e);
            }
        }
    }
}

/// Validate that a URL's host matches the expected base URL
fn validate_url_host(url: &str, expected_base_url: &str) -> Result<()> {
    // Parse both URLs to compare their hosts
    let input_url = Url::parse(url)
        .map_err(|_| anyhow::anyhow!("Invalid URL format"))?;
    let expected_url = Url::parse(expected_base_url)
        .map_err(|_| anyhow::anyhow!("Invalid expected base URL format"))?;
    
    // Compare the host portions
    if input_url.host_str() != expected_url.host_str() {
        bail!(
            "URL host '{}' does not match expected host '{}'. Only images from {} can be deleted.",
            input_url.host_str().unwrap_or("unknown"),
            expected_url.host_str().unwrap_or("unknown"),
            expected_base_url
        );
    }
    
    Ok(())
}

/// Extract UUID from input string (handles both direct UUIDs and URLs)
/// For URLs, validates that the host matches the expected base URL
fn extract_uuid_from_input(input: &str, expected_base_url: &str) -> Result<String> {
    // First, check if it's a direct UUID (36 characters, contains dashes)
    if input.len() == 36 && input.chars().filter(|&c| c == '-').count() == 4 {
        // Validate it looks like a UUID
        if uuid::Uuid::parse_str(input).is_ok() {
            return Ok(input.to_string());
        }
    }

    // If not a direct UUID, try to extract from URL
    if input.starts_with("http://") || input.starts_with("https://") {
        // Validate the host matches our expected base URL
        validate_url_host(input, expected_base_url)?;
        
        // Extract filename from URL
        if let Some(filename) = input.split('/').next_back() {
            // Remove file extension if present
            let name_without_ext = filename.split('.').next().unwrap_or(filename);

            // Handle gallery image format: {uuid}.{index}.jpg -> extract just the UUID part
            let uuid_part = name_without_ext
                .split('.')
                .next()
                .unwrap_or(name_without_ext);

            // Validate it's a UUID
            if uuid_part.len() == 36 && uuid::Uuid::parse_str(uuid_part).is_ok() {
                return Ok(uuid_part.to_string());
            }
        }
    }

    bail!(
        "Invalid UUID or URL format. Provide either a UUID (36 characters with dashes) or an image URL"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BASE_URL: &str = "https://example.com";

    #[test]
    fn test_extract_uuid_from_direct_uuid() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let result = extract_uuid_from_input(uuid, TEST_BASE_URL).unwrap();
        assert_eq!(result, uuid);
    }

    #[test]
    fn test_extract_uuid_from_url() {
        let url = "https://example.com/550e8400-e29b-41d4-a716-446655440000.jpg";
        let result = extract_uuid_from_input(url, TEST_BASE_URL).unwrap();
        assert_eq!(result, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn test_extract_uuid_from_gallery_url() {
        let url = "https://example.com/550e8400-e29b-41d4-a716-446655440000.1.jpg";
        let result = extract_uuid_from_input(url, TEST_BASE_URL).unwrap();
        assert_eq!(result, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn test_invalid_uuid() {
        let result = extract_uuid_from_input("not-a-uuid", TEST_BASE_URL);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_url() {
        let result = extract_uuid_from_input("https://example.com/not-a-uuid.jpg", TEST_BASE_URL);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_input() {
        let result = extract_uuid_from_input("", TEST_BASE_URL);
        assert!(result.is_err());
    }

    #[test]
    fn test_url_host_validation_success() {
        let url = "https://example.com/550e8400-e29b-41d4-a716-446655440000.jpg";
        let result = extract_uuid_from_input(url, TEST_BASE_URL);
        assert!(result.is_ok());
    }

    #[test]
    fn test_url_host_validation_failure() {
        let url = "https://malicious.com/550e8400-e29b-41d4-a716-446655440000.jpg";
        let result = extract_uuid_from_input(url, TEST_BASE_URL);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not match expected host"));
    }

    #[test]
    fn test_url_host_validation_different_subdomain() {
        let url = "https://sub.example.com/550e8400-e29b-41d4-a716-446655440000.jpg";
        let result = extract_uuid_from_input(url, TEST_BASE_URL);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not match expected host"));
    }
}
