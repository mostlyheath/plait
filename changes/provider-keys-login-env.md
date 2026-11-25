Fixed
API keys your shell exports, used as $NAME or ${NAME} in ~/.pi/agent/models.json or by built-in providers, now work in the app. They're used only for provider requests and never reach commands or Git hooks. Changing a custom endpoint's server forgets its saved key.
