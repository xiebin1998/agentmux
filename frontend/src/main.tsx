import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ThemeProvider } from "./theme";
import { ProvidersProvider } from "./providers";
import "./styles/theme.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <ProvidersProvider>
        <App />
      </ProvidersProvider>
    </ThemeProvider>
  </React.StrictMode>
);
