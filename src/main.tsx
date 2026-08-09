import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

const root = document.getElementById("root");
if (!root) {
  document.body.innerHTML = '<div style="color:red;padding:20px;font-family:monospace;">ERROR: #root element not found</div>';
} else {
  try {
    ReactDOM.createRoot(root).render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    );
  } catch (e) {
    root.innerHTML = `<div style="color:red;padding:20px;font-family:monospace;">ERROR: ${e}</div>`;
  }
}
