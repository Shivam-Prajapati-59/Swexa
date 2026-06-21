// Routing optimizer — will contain the math engine that simulates
// swap outputs through each RouteCandidate to pick the best route.
//
// Planned:
// - calculate_output() per pool type (CPMM x*y=k, CLMM tick math, DLMM bin math)
// - simulate_route() that chains outputs through a RouteCandidate's steps
// - find_best_route() that compares all candidates and returns the highest output
