gen-client:
	@command -v openapi-generator-cli >/dev/null 2>&1 || npx @openapitools/openapi-generator-cli generate \
		-i openapi.yaml \
		-g typescript-fetch \
		-o clients/ts \
		-c openapi-generator-config.yaml
	@openapi-generator-cli generate \
		-i openapi.yaml \
		-g typescript-fetch \
		-o clients/ts \
		-c openapi-generator-config.yaml
