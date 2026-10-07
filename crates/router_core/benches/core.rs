//! Benchmarks of the hot paths of the core library. Run with:
//! `cargo bench -p router_core --features test-support`

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use petgraph::graph::NodeIndex;
use router_core::ansiblex::{self, BridgeRules};
use router_core::overlay;
use router_core::route::{Mode, Router, RouterOptions};
use router_core::test_support::{fixture, hole, sde_dir};
use router_core::universe::Universe;
use std::hint::black_box;

fn load() -> Universe {
    Universe::from_sde(router_core::sde::load(&sde_dir()).unwrap())
}

fn with_overlays() -> Universe {
    let mut uni = load();
    overlay::load_bridges(&mut uni, &fixture("ansiblex.txt")).unwrap();
    // Wormholes between far systems, so a route can use them.
    let id = |n: &str| uni.system(uni.exact(n).unwrap()).id;
    let holes: Vec<_> = [("Jita", "Amarr"), ("Dodixie", "Hek"), ("Rens", "Perimeter")].iter().map(|&(a, b)| hole(id(a), id(b))).collect();
    uni.add_wormholes(&holes);
    uni
}

fn node(uni: &Universe, n: &str) -> NodeIndex {
    uni.exact(n).unwrap()
}

fn bench(c: &mut Criterion) {
    ansiblex::init(&sde_dir()).unwrap();
    let uni = with_overlays();
    let rules = BridgeRules { capital: uni.exact("JK-Q77"), hull: ansiblex::find_hull("Sin"), max_cap: None };
    let router = |mode| Router::new(&uni, RouterOptions { mode, rules, ..Default::default() });

    let mut g = c.benchmark_group("load");
    g.sample_size(20);
    g.bench_function("sde_parse", |b| b.iter(|| router_core::sde::load(&sde_dir()).unwrap()));
    g.bench_function("universe_build", |b| {
        b.iter_batched(|| router_core::sde::load(&sde_dir()).unwrap(), Universe::from_sde, BatchSize::LargeInput)
    });
    g.finish();

    let mut g = c.benchmark_group("router");
    g.bench_function("new_shortest", |b| b.iter(|| router(Mode::Shortest)));
    let r = router(Mode::Shortest);
    let (jita, amarr, dodixie) = (node(&uni, "Jita"), node(&uni, "Amarr"), node(&uni, "Dodixie"));
    let far = node(&uni, "UALX-3");
    g.bench_function("route_jita_amarr_top1", |b| b.iter(|| r.routes(black_box(&[jita, amarr]), 1).unwrap()));
    g.bench_function("route_jita_amarr_top5", |b| b.iter(|| r.routes(black_box(&[jita, amarr]), 5).unwrap()));
    g.bench_function("route_jita_amarr_top20", |b| b.iter(|| r.routes(black_box(&[jita, amarr]), 20).unwrap()));
    g.bench_function("route_jita_ualx_top5", |b| b.iter(|| r.routes(black_box(&[jita, far]), 5).unwrap()));
    let long: Vec<_> = ["Jita", "Rens", "Hek", "Dodixie", "Amarr", "UALX-3"].iter().map(|n| node(&uni, n)).collect();
    g.bench_function("chain_6_waypoints_top3", |b| b.iter(|| r.routes(black_box(&long), 3).unwrap()));
    let hs = router(Mode::PreferHighsec);
    g.bench_function("prefer_highsec_jita_amarr_top5", |b| b.iter(|| hs.routes(black_box(&[jita, amarr]), 5).unwrap()));
    g.bench_function("yen_k50_jita_dodixie", |b| b.iter(|| r.k_shortest(black_box(jita), dodixie, 50)));
    let hubs: Vec<_> = ["Jita", "Amarr", "Dodixie", "Hek", "Rens"].iter().map(|n| node(&uni, n)).collect();
    g.bench_function("jumps_to_hubs", |b| b.iter(|| r.jumps_to(black_box(jita), &hubs)));
    g.finish();

    let mut g = c.benchmark_group("optimize");
    g.sample_size(10);
    let names = [
        "Rens",
        "Hek",
        "Dodixie",
        "Perimeter",
        "Tash-Murkon Prime",
        "Oursulaert",
        "Turnur",
        "Pator",
        "Ashab",
        "Sivala",
        "Aunia",
        "Kisogo",
        "Otela",
        "Ommare",
    ];
    for m in [6usize, 10, 14] {
        let mut w = vec![jita];
        w.extend(names.iter().cycle().take(m).map(|n| node(&uni, n)));
        w.push(amarr);
        w.dedup();
        g.bench_function(format!("held_karp_{m}_midpoints"), |b| b.iter(|| r.optimize(black_box(&w)).unwrap()));
    }
    g.finish();

    let mut g = c.benchmark_group("universe");
    g.bench_function("search_ama", |b| b.iter(|| uni.search(black_box("ama"), 20)));
    g.bench_function("resolve_exact", |b| b.iter(|| uni.resolve(black_box("jita"))));
    g.bench_function("distance_ly", |b| b.iter(|| uni.distance_ly(jita, amarr)));
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
