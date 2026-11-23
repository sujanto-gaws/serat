import { createPinia } from "pinia";
import { createApp } from "vue";

import App from "./App.vue";
import "./tokens.css";
import "./style.css";

createApp(App).use(createPinia()).mount("#app");
