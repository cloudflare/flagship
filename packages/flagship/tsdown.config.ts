import { defineConfig } from 'tsdown';

export default defineConfig({
	entry: {
		index: 'src/index.ts',
		server: 'src/server.ts',
		web: 'src/web.ts',
	},
	format: ['esm', 'cjs'],
	dts: true,
	clean: true,
	deps: {
		onlyBundle: false,
	},
});
