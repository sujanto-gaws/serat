import { createPinia } from "pinia";
import { createApp } from "vue";

import App from "./App.vue";
import "./styles/tokens.css";
import "./styles/fonts.css";
import "./styles/prose.css";
import "./style.css";

createApp(App).use(createPinia()).mount("#app");
