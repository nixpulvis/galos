use eframe::{egui, epi};
use egui::{Grid, TextStyle};
use egui::plot::{Points, Plot, Value, Values};
use egui::widgets::plot::{Legend, Corner};
use async_std::task;
use itertools::Itertools;
use galos_db::{Database, systems::System};

#[cfg_attr(feature = "persistence", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "persistence", serde(default))]
pub struct Galos {
    start: String,
    end: String,

    #[cfg_attr(feature = "persistence", serde(skip))]
    radius: f32,

    table: Vec<Vec<String>>,
    plot: Vec<(f64, f64, f64)>,
}

impl Default for Galos {
    fn default() -> Self {
        Self {
            start: "Sol".into(),
            end: "Meliae".into(),
            radius: 10.,
            table: vec![],
            plot: vec![],
        }
    }
}

impl epi::App for Galos {
    fn name(&self) -> &str {
        "galos (gui)"
    }

    fn setup(
        &mut self,
        _ctx: &egui::CtxRef,
        _frame: &mut epi::Frame<'_>,
        _storage: Option<&dyn epi::Storage>)
    {
        #[cfg(feature = "persistence")]
        if let Some(storage) = _storage {
            *self = epi::get_value(storage, epi::APP_KEY).unwrap_or_default()
        }
    }

    #[cfg(feature = "persistence")]
    fn save(&mut self, storage: &mut dyn epi::Storage) {
        epi::set_value(storage, epi::APP_KEY, self);
    }

    fn update(&mut self, ctx: &egui::CtxRef, _frame: &mut epi::Frame<'_>) {
        let Self { start, end, radius, table, plot } = self;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("*Start: ");
                ui.text_edit_singleline(start);
                ui.label(" End: ");
                ui.text_edit_singleline(end);
            });

            ui.horizontal(|ui| {
                if ui.button("Route").clicked() {
                    table.clear();
                    plot.clear();

                    let (route, cost) = task::block_on(async {
                        let db = Database::new().await.unwrap();
                        let start = System::fetch_by_name(&db, start).await.unwrap();
                        let end = System::fetch_by_name(&db, end).await.unwrap();
                        start.route_to(&db, &end, *radius as f64).unwrap().unwrap()
                    });

                    let mut gross = 0.;
                    for (a, b) in route[..].into_iter().tuple_windows() {
                        let d = a.distance(&b);
                        table.push(vec![
                            format!("{}", a.name),
                            format!("{}", b.name),
                            format!("{:.2} Ly", d),
                        ]);
                        plot.push((b.position.x, b.position.y, b.position.z));
                        gross += d;
                    }

                    table.push(vec![
                        format!("JUMPS {}", cost),
                        format!("PATH {:.2} Ly", gross),
                        format!("DISTANCE {:.2} Ly",
                            route[0].distance(&route.last().expect("valid route"))),
                    ]);
                }

                if ui.button("Lookup").clicked() {
                    table.clear();

                    let system = task::block_on(async {
                        let db = Database::new().await.unwrap();
                        System::fetch_by_name(&db, start).await.unwrap()
                    });

                    table.push(vec!["address".into(),           format!("{}", system.address)]);
                    table.push(vec!["name".into(),              system.name.into()]);
                    table.push(vec!["position".into(),          format!("{:?}", system.position)]);
                    table.push(vec!["population".into(),        format!("{}",   system.population)]);
                    table.push(vec!["security".into(),          format!("{:?}", system.security)]);
                    table.push(vec!["government".into(),        format!("{:?}", system.government)]);
                    table.push(vec!["allegiance".into(),        format!("{:?}", system.allegiance)]);
                    table.push(vec!["primary_economy".into(),   format!("{:?}", system.primary_economy)]);
                    table.push(vec!["secondary_economy".into(), format!("{:?}", system.secondary_economy)]);
                    table.push(vec!["updated_at".into(),        format!("{}", system.updated_at)]);
                }

                if ui.button("Map").clicked() {
                    plot.clear();

                    let systems = task::block_on(async {
                        let db = Database::new().await.unwrap();
                        System::fetch_in_range_like_name(&db, *radius as f64, start).await.unwrap()
                    });

                    for system in systems {
                        plot.push((system.position.x, system.position.y, system.position.z))
                    }
                }

                if ui.button("-").clicked() { *radius -= 5.0; }
                if ui.button("+").clicked() { *radius += 5.0; }
                ui.add(egui::Slider::new(radius, 0.0..=500.).text("Ly"));
            });

            const RADIUS: f32 = 3.0;
            const SIZE: f32 =  256.0;

            Grid::new("table").striped(true).show(ui, |ui| {
                for row in table.iter() {
                    for cell in row {
                        ui.label(format!("{}", cell));
                    }
                    ui.end_row();
                }
            });
            ui.horizontal(|ui| {
                let xy  = plot.iter().map(|(x,y,_)| Value::new(*x, *y));
                let sxy = Points::new(Values::from_values_iter(xy)).radius(RADIUS).name("x;y;");
                ui.add(Plot::new("plot xy").points(sxy).width(SIZE).height(SIZE)
                .legend(Legend::default()));
                let xz  = plot.iter().map(|(x,_,z)| Value::new(*x, *z));
                let sxz = Points::new(Values::from_values_iter(xz)).radius(RADIUS).name("x=x; y=z;");
                ui.add(Plot::new("plot xz").points(sxz).width(SIZE).height(SIZE)
                    .legend(Legend::default()));
                let zy  = plot.iter().map(|(_,y,z)| Value::new(-*y, *z));
                let szy = Points::new(Values::from_values_iter(zy)).radius(RADIUS).name("x=-y;y=z");
                ui.add(Plot::new("plot zy").points(szy).width(SIZE).height(SIZE)
                    .legend(Legend::default()));
            });
        });
    }
}
