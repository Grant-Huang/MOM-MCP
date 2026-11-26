use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::Dimensions;

#[derive(Serialize, Deserialize)]
pub(super) struct Placement {
    x_mm: u64,
    y_mm: u64,
    length_mm: u64,
    width_mm: u64,
}

pub(super) trait LayoutService {
    fn place(
        &self,
        stock: Dimensions,
        footprints: &BTreeMap<String, Dimensions>,
    ) -> Option<BTreeMap<String, Placement>>;
}

pub(super) struct RowLayout;

impl LayoutService for RowLayout {
    fn place(
        &self,
        stock: Dimensions,
        footprints: &BTreeMap<String, Dimensions>,
    ) -> Option<BTreeMap<String, Placement>> {
        let mut next_x = 0;
        footprints
            .iter()
            .map(|(order, shape)| {
                let placement = Placement {
                    x_mm: next_x,
                    y_mm: 0,
                    length_mm: shape.length_mm,
                    width_mm: shape.width_mm,
                };
                next_x += shape.length_mm;
                (next_x <= stock.length_mm && shape.width_mm <= stock.width_mm)
                    .then(|| (order.clone(), placement))
            })
            .collect()
    }
}

pub(super) fn validated_plan(
    service: &dyn LayoutService,
    stock: Dimensions,
    footprints: &BTreeMap<String, Dimensions>,
) -> Result<BTreeMap<String, Placement>, &'static str> {
    let plan = service
        .place(stock, footprints)
        .ok_or("NEST_LAYOUT_UNAVAILABLE")?;
    let bounds = |part: &Placement| {
        part.x_mm
            .checked_add(part.length_mm)
            .zip(part.y_mm.checked_add(part.width_mm))
    };
    if plan.len() != footprints.len()
        || plan.iter().any(|(order, part)| {
            !footprints.get(order).is_some_and(|shape| {
                shape.length_mm == part.length_mm && shape.width_mm == part.width_mm
            }) || !bounds(part).is_some_and(|(x, y)| x <= stock.length_mm && y <= stock.width_mm)
        })
    {
        return Err("NEST_LAYOUT_INVALID");
    }
    let parts: Vec<_> = plan.values().collect();
    for (index, a) in parts.iter().enumerate() {
        for b in &parts[index + 1..] {
            if a.x_mm < b.x_mm + b.length_mm
                && b.x_mm < a.x_mm + a.length_mm
                && a.y_mm < b.y_mm + b.width_mm
                && b.y_mm < a.y_mm + a.width_mm
            {
                return Err("NEST_LAYOUT_INVALID");
            }
        }
    }
    Ok(plan)
}

pub(super) fn occupied_length(plan: &BTreeMap<String, Placement>) -> u64 {
    plan.values()
        .map(|part| part.x_mm + part.length_mm)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OverlappingLayout;

    impl LayoutService for OverlappingLayout {
        fn place(
            &self,
            _: Dimensions,
            footprints: &BTreeMap<String, Dimensions>,
        ) -> Option<BTreeMap<String, Placement>> {
            Some(
                footprints
                    .iter()
                    .map(|(order, part)| {
                        (
                            order.clone(),
                            Placement {
                                x_mm: 0,
                                y_mm: 0,
                                length_mm: part.length_mm,
                                width_mm: part.width_mm,
                            },
                        )
                    })
                    .collect(),
            )
        }
    }

    #[test]
    fn rejects_overlapping_service_output() {
        let footprints = BTreeMap::from([
            ("A".into(), Dimensions::new(30, 20)),
            ("B".into(), Dimensions::new(30, 20)),
        ]);
        assert!(matches!(
            validated_plan(&OverlappingLayout, Dimensions::new(60, 30), &footprints),
            Err("NEST_LAYOUT_INVALID")
        ));
    }
}
