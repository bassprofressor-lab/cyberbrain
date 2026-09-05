// Records which transport the bundle in dist/ was built with, so the Rust build script can
// refuse to embed a mock one into a release binary. A comment in the source would not
// survive into dist/, and grepping the minified bundle for a marker string is guesswork.
import { writeFileSync } from "node:fs";

const transport = process.env.VITE_API === "mock" ? "mock" : "http";
writeFileSync("dist/transport.txt", `${transport}\n`);
console.log(`dist/transport.txt: ${transport}`);
