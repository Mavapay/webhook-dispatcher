// Update webhook URL in the info section
function updateWebhookUrl() {
  const serverUrl = window.location.origin;
  const infoUrl = document.getElementById("webhookInfoUrl");
  const sourceUrl = document.getElementById("webhookSourceUrl");
  if (infoUrl) infoUrl.textContent = `${serverUrl}/webhook/{source}`;
  if (sourceUrl) sourceUrl.textContent = `${serverUrl}/webhook/fincra`;
}

// Load endpoints from server
async function loadEndpoints() {
  try {
    const response = await fetch("/endpoints");
    const endpoints = await response.json();
    displayEndpoints(endpoints);
  } catch (error) {
    console.error("Error loading endpoints:", error);
    showNotification("Error loading endpoints", "error");
  }
}

// Display endpoints in the list
function displayEndpoints(endpoints) {
  const endpointList = document.getElementById("endpointList");
  endpointList.innerHTML = "";

  endpoints.forEach((endpoint) => {
    const div = document.createElement("div");
    div.className = "endpoint-item";
    const sourceLabel = endpoint.source
      ? `<span class="endpoint-source">Source: ${escapeHtml(endpoint.source)}</span>`
      : `<span class="endpoint-source endpoint-source-all">Source: All</span>`;

    div.innerHTML = `
            <div class="endpoint-info">
                <div class="endpoint-name">${escapeHtml(endpoint.name)}</div>
                <div class="endpoint-url">${escapeHtml(endpoint.url)}</div>
                ${sourceLabel}
            </div>
            <div class="endpoint-controls">
                <label class="toggle-switch">
                    <input type="checkbox"
                           ${endpoint.is_active ? "checked" : ""}
                           onchange="toggleEndpoint('${endpoint.id}', this.checked)">
                    <span class="slider"></span>
                </label>
                <button class="delete" onclick="deleteEndpoint('${endpoint.id}')">Delete</button>
            </div>
        `;
    endpointList.appendChild(div);
  });
}

// Add new endpoint
async function addEndpoint() {
  const urlInput = document.getElementById("webhookUrl");
  const nameInput = document.getElementById("webhookName");
  if (!urlInput || !nameInput) {
    showNotification("Form elements not found - please hard refresh (Ctrl+Shift+R)", "error");
    return;
  }
  const url = urlInput.value.trim();
  const name = nameInput.value.trim();

  if (!url || !name) {
    showNotification("Please fill in all fields", "error");
    return;
  }

  try {
    const response = await fetch("/endpoints", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        url,
        name,
        is_active: false,
      }),
    });

    if (!response.ok) {
      const errorData = await response.json();
      throw new Error(errorData.error || "Failed to add endpoint");
    }

    const endpoints = await response.json();
    displayEndpoints(endpoints);
    urlInput.value = "";
    nameInput.value = "";
    showNotification("Endpoint added successfully", "success");
  } catch (error) {
    console.error("Error adding endpoint:", error);
    showNotification(error.message, "error");
  }
}

// Toggle endpoint active status
async function toggleEndpoint(id, isActive) {
  try {
    const response = await fetch(`/endpoints/${id}/status`, {
      method: "PUT",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ is_active: isActive }),
    });

    if (!response.ok) {
      throw new Error("Failed to update endpoint status");
    }

    showNotification(
      `Endpoint ${isActive ? "activated" : "deactivated"} successfully`,
      "success",
    );
  } catch (error) {
    console.error("Error updating endpoint:", error);
    showNotification("Error updating endpoint status", "error");
    // Revert the checkbox state
    await loadEndpoints();
  }
}

// Delete endpoint
async function deleteEndpoint(id) {
  if (!confirm("Are you sure you want to delete this endpoint?")) {
    return;
  }

  try {
    const response = await fetch(`/endpoints/${id}`, {
      method: "DELETE",
    });

    if (response.ok) {
      const endpoints = await response.json();
      displayEndpoints(endpoints);
      showNotification("Endpoint deleted successfully", "success");
    } else {
      throw new Error("Failed to delete endpoint");
    }
  } catch (error) {
    console.error("Error deleting endpoint:", error);
    showNotification("Error deleting endpoint", "error");
  }
}

// Helper function to escape HTML
function escapeHtml(unsafe) {
  return unsafe
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

// Show notification
function showNotification(message, type = "info") {
  const notificationDiv = document.createElement("div");
  notificationDiv.className = `notification ${type}`;
  notificationDiv.textContent = message;

  document.body.appendChild(notificationDiv);

  // Remove notification after 3 seconds
  setTimeout(() => {
    notificationDiv.remove();
  }, 3000);
}

// Initialize
document.addEventListener("DOMContentLoaded", () => {
  updateWebhookUrl();
  loadEndpoints();
});
