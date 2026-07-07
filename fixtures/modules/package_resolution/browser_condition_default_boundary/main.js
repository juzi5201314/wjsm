// FixtureRunner uses default module resolution options, so this case proves
// the browser condition is opt-in and does not affect generated fixtures.
import { value } from 'pkg';

console.log(value);
