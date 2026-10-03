//! Read-only Windows foreground-window inspection for fullscreen avoidance.
//! Uses visible DWM bounds, in physical pixels (GetWindowRect includes borders).
#[derive(Debug,Clone,Copy,Default,PartialEq,Eq,serde::Serialize,serde::Deserialize)]
#[repr(C)]
pub struct Rect { pub left:i32,pub top:i32,pub right:i32,pub bottom:i32 }
pub fn covers_display(window:Rect,screen:Rect,style:u32,class:&str,own_process:bool)->bool {
    if own_process || ["Progman","WorkerW","Shell_TrayWnd","Shell_SecondaryTrayWnd"].contains(&class) || style&0x00c00000!=0 {return false}
    window.left<=screen.left && window.top<=screen.top && window.right>=screen.right && window.bottom>=screen.bottom
}
#[cfg(windows)]
pub fn fullscreen_on(screen:Rect)->bool {
    use std::ffi::c_void;
    #[link(name="user32")] extern "system" {
        fn GetForegroundWindow()->*mut c_void;
        fn GetWindowThreadProcessId(window:*mut c_void,pid:*mut u32)->u32;
        fn GetWindowLongW(window:*mut c_void,index:i32)->i32;
        fn GetClassNameW(window:*mut c_void,class:*mut u16,max:i32)->i32;
        fn IsWindowVisible(window:*mut c_void)->i32;
        fn IsIconic(window:*mut c_void)->i32;
    }
    #[link(name="dwmapi")] extern "system" {fn DwmGetWindowAttribute(window:*mut c_void,attribute:u32,value:*mut c_void,size:u32)->i32;}
    // Every pointer below addresses a fixed-size live local buffer. No window
    // text, app contents, or process arguments are read.
    unsafe {
        let window=GetForegroundWindow();if window.is_null()||IsWindowVisible(window)==0||IsIconic(window)!=0{return false}
        let mut pid=0;GetWindowThreadProcessId(window,&mut pid);
        let mut class=[0u16;256];let count=GetClassNameW(window,class.as_mut_ptr(),class.len() as i32);
        let class=String::from_utf16_lossy(&class[..count.max(0) as usize]);
        let mut bounds=Rect::default();
        if DwmGetWindowAttribute(window,9,(&mut bounds as *mut Rect).cast(),std::mem::size_of::<Rect>() as u32)<0{return false}
        covers_display(bounds,screen,GetWindowLongW(window,-16) as u32,&class,pid==std::process::id())
    }
}
#[cfg(not(windows))]pub fn fullscreen_on(_:Rect)->bool{false}

/// Preserve a rail's relative position when moving between differently sized
/// displays, including negative desktop coordinates and a change in DPI.
pub fn transfer(point:(i32,i32),old:Rect,new:Rect,rail:(f64,f64),old_scale:f64,new_scale:f64)->(i32,i32) {
    let axis=|p:i32,lo:i32,hi:i32,nlo:i32,nhi:i32,length:f64| {
        let old_room=((hi-lo) as f64-length*old_scale).max(1.0);
        let fraction=((p-lo) as f64/old_room).clamp(0.0,1.0);
        nlo+(((nhi-nlo) as f64-length*new_scale).max(0.0)*fraction).round() as i32
    };
    (axis(point.0,old.left,old.right,new.left,new.right,rail.0),axis(point.1,old.top,old.bottom,new.top,new.bottom,rail.1))
}

pub struct Landing {pub edge:crate::PanelEdge,pub docked:bool,pub origin:(i32,i32),pub grab:(f64,f64),pub rail:(f64,f64)}
/// Mirrors FloatingPanel's drag: dock during movement; quarter turns centre
/// under the pointer, while dock/float length changes keep the rings in place.
pub fn drag_landing(pointer:(f64,f64),grab:(f64,f64),rail:(f64,f64),edge:crate::PanelEdge,was_docked:bool,screen:Rect,scale:f64,flare:f64)->Landing {
    use crate::PanelEdge::{Left,Right,Top};
    let clamp_origin=|x:f64,y:f64,w:f64,h:f64|(x.clamp(screen.left as f64,(screen.right as f64-w).max(screen.left as f64)),y.clamp(screen.top as f64,(screen.bottom as f64-h).max(screen.top as f64)));
    let wanted=clamp_origin(pointer.0-grab.0*scale,pointer.1-grab.1*scale,rail.0*scale,rail.1*scale);
    let distance=24.*scale;
    let dock=if pointer.1-screen.top as f64<=distance{Some(Top)}else if wanted.0-screen.left as f64<=distance{Some(Left)}else if screen.right as f64-(wanted.0+rail.0*scale)<=distance{Some(Right)}else{None};
    let landing=dock.unwrap_or(if pointer.0<(screen.left+screen.right) as f64/2.{Left}else{Right});
    let turned=(landing==Top)!=(edge==Top);
    let mut size=if turned{(rail.1,rail.0)}else{rail};
    let delta=(i32::from(dock.is_some())-i32::from(was_docked)) as f64*2.*flare;
    if landing==Top{size.0+=delta}else{size.1+=delta}
    let grab=if turned{(size.0/2.,size.1/2.)}else{(grab.0-(rail.0-size.0)/2.,grab.1-(rail.1-size.1)/2.)};
    let mut origin=clamp_origin(pointer.0-grab.0*scale,pointer.1-grab.1*scale,size.0*scale,size.1*scale);
    match dock{Some(Left)=>origin.0=screen.left as f64,Some(Right)=>origin.0=(screen.right as f64-size.0*scale).max(screen.left as f64),Some(Top)=>origin.1=screen.top as f64,None=>{}}
    Landing{edge:landing,docked:dock.is_some(),origin:(origin.0.round() as i32,origin.1.round() as i32),grab,rail:size}
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Display {
    pub id: String,
    pub bounds: Rect,
    pub work_area: Rect,
    pub scale: f64,
}

impl Display {
    pub fn contains(&self, point: (f64, f64)) -> bool {
        point.0 >= self.bounds.left as f64 && point.0 < self.bounds.right as f64
            && point.1 >= self.bounds.top as f64 && point.1 < self.bounds.bottom as f64
    }
}

fn valid_scale(scale: f64) -> f64 { if scale.is_finite() && scale > 0.0 { scale } else { 1.0 } }
fn unit(value: f64) -> f64 { if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.5 } }

/// Fractions describe the rail's available travel, never the transparent
/// window's frame. All screen coordinates here are physical Windows pixels.
pub fn ratios(origin: (i32, i32), display: &Display, rail: (f64, f64)) -> (f64, f64) {
    let scale = valid_scale(display.scale);
    let axis = |p: i32, min: i32, max: i32, length: f64| {
        let room = (max - min) as f64 - length * scale;
        if room > 0.0 { unit((p - min) as f64 / room) } else { 0.5 }
    };
    (axis(origin.0, display.work_area.left, display.work_area.right, rail.0),
     axis(origin.1, display.work_area.top, display.work_area.bottom, rail.1))
}

/// An absolute position belongs to one physical coordinate space. Old files
/// have no space metadata, so adopt their point only on the preferred display
/// (or the display containing the legacy point). This is deliberately checked
/// before converting an empty startup rail to a normalized position.
pub fn absolute_anchor(settings: &crate::settings::PanelSettings, display: &Display) -> Option<(i32,i32)> {
    settings.rail_position.filter(|origin| match &settings.rail_space {
        Some(space)=>space==display,
        None=>(settings.display.is_none()||settings.display.as_deref()==Some(display.id.as_str()))
            &&display.contains((origin.0 as f64,origin.1 as f64)),
    })
}

/// Same-display metrics changes preserve the chosen rail top-left. Only a
/// display/work-area/DPI change recalls ratios. Clamp actual overflow, pin the
/// docked axis, and keep the ordinary floating gap in physical pixels.
pub fn layout_origin(settings: &crate::settings::PanelSettings, memory: &crate::settings::DisplayPlacement,
    display: &Display, rail: (f64,f64)) -> (i32,i32) {
    let Some(origin)=absolute_anchor(settings,display) else{return restored_origin(memory,display,rail)};
    let area=display.work_area;let scale=valid_scale(display.scale);
    let room_x=((area.right-area.left) as f64-rail.0*scale).max(0.0);
    let room_y=((area.bottom-area.top) as f64-rail.1*scale).max(0.0);
    let mut x=(origin.0 as f64).clamp(area.left as f64,area.left as f64+room_x);
    let mut y=(origin.1 as f64).clamp(area.top as f64,area.top as f64+room_y);
    match memory.dock_edge {
        Some(crate::PanelEdge::Left)=>x=area.left as f64,
        Some(crate::PanelEdge::Right)=>x=area.left as f64+room_x,
        Some(crate::PanelEdge::Top)=>y=display.bounds.top as f64,
        None=>{
            let gap=(32.0*scale).min(room_x/2.0);
            x=x.clamp(area.left as f64+gap,area.left as f64+room_x-gap);
        },
    }
    (x.round() as i32,y.round() as i32)
}

/// Record the actual space even during a hot-plug fallback, without changing
/// its preferred identity or overwriting that disconnected display's record.
pub fn record_layout(settings: &mut crate::settings::PanelSettings, display: &Display,
    memory: &crate::settings::DisplayPlacement, origin: (i32,i32), rail: (f64,f64)) {
    settings.rail_position=Some(origin);
    settings.rail_space=Some(display.clone());
    if settings.display.is_none(){settings.display=Some(display.id.clone());}
    if settings.display.as_deref()==Some(display.id.as_str()){
        let (horizontal_ratio,vertical_ratio)=ratios(origin,display,rail);
        settings.per_display.insert(display.id.clone(),crate::settings::DisplayPlacement {
            horizontal_ratio,vertical_ratio,..memory.clone()
        });
    }
}

pub fn restored_origin(memory: &crate::settings::DisplayPlacement, display: &Display, rail: (f64, f64)) -> (i32, i32) {
    let area = display.work_area;
    let scale = valid_scale(display.scale);
    let room_x = ((area.right - area.left) as f64 - rail.0 * scale).max(0.0);
    let room_y = ((area.bottom - area.top) as f64 - rail.1 * scale).max(0.0);
    let mut x = area.left as f64 + unit(memory.horizontal_ratio) * room_x;
    let mut y = area.top as f64 + unit(memory.vertical_ratio) * room_y;
    match memory.dock_edge {
        Some(crate::PanelEdge::Left) => x = area.left as f64,
        Some(crate::PanelEdge::Right) => x = area.left as f64 + room_x,
        Some(crate::PanelEdge::Top) => y = display.bounds.top as f64,
        None => {
            // A floating rail must leave a gap instead of looking docked.
            let gap = (32.0 * scale).min(room_x / 2.0);
            x = x.clamp(area.left as f64 + gap, area.left as f64 + room_x - gap);
        }
    }
    (x.round() as i32, y.round() as i32)
}

pub fn memory_edge(memory: &crate::settings::DisplayPlacement) -> crate::PanelEdge {
    memory.dock_edge.or(memory.facing_edge.filter(|edge|*edge!=crate::PanelEdge::Top))
        .unwrap_or_else(|| if unit(memory.horizontal_ratio) < 0.5 { crate::PanelEdge::Left } else { crate::PanelEdge::Right })
}

pub fn rail_for_memory(rail: (f64, f64), current_edge: crate::PanelEdge, current_docked: bool,
    memory: &crate::settings::DisplayPlacement, flare: f64) -> (f64, f64) {
    let target_edge = memory_edge(memory);
    let mut size = if (current_edge == crate::PanelEdge::Top) != (target_edge == crate::PanelEdge::Top) { (rail.1, rail.0) } else { rail };
    let delta = (i32::from(memory.dock_edge.is_some()) - i32::from(current_docked)) as f64 * 2.0 * flare;
    if target_edge == crate::PanelEdge::Top { size.0 = (size.0 + delta).max(1.0); } else { size.1 = (size.1 + delta).max(1.0); }
    size
}

pub fn remember(settings: &mut crate::settings::PanelSettings, display: &Display, origin: (i32, i32), rail: (f64, f64)) {
    let (horizontal_ratio, vertical_ratio) = ratios(origin, display, rail);
    settings.display = Some(display.id.clone());
    settings.rail_position = Some(origin);
    settings.rail_space = Some(display.clone());
    settings.per_display.insert(display.id.clone(), crate::settings::DisplayPlacement {
        dock_edge: settings.dock_edge, facing_edge: settings.facing_edge,
        horizontal_ratio, vertical_ratio,
    });
}

/// First-run/old-file migration and hot-plug restoration. The preferred id is
/// retained while absent; reconnecting that device recovers its own memory.
pub fn resolve(settings: &crate::settings::PanelSettings, displays: &[Display], primary: Option<&str>, rail: (f64, f64))
    -> Option<(Display, crate::settings::DisplayPlacement)> {
    let preferred = settings.display.as_deref().and_then(|id| displays.iter().find(|display| display.id == id));
    let legacy = if settings.display.is_none() {
        settings.rail_position.and_then(|origin| displays.iter().find(|display| display.contains((origin.0 as f64, origin.1 as f64))))
    } else { None };
    let target = preferred.or(legacy).or_else(|| primary.and_then(|id| displays.iter().find(|display| display.id == id))).or_else(|| displays.first())?;
    let memory = settings.per_display.get(&target.id).cloned()
        .or_else(|| settings.display.as_ref().and_then(|id| settings.per_display.get(id)).cloned())
        .unwrap_or_else(|| {
            let mut memory = crate::settings::DisplayPlacement {
                dock_edge: settings.dock_edge.or_else(|| settings.rail_position.is_none().then_some(crate::PanelEdge::Right)),
                facing_edge: settings.facing_edge, ..Default::default()
            };
            if let Some(origin) = settings.rail_position {
                let (h, v) = ratios(origin, target, rail);
                memory.horizontal_ratio = h; memory.vertical_ratio = v;
                // Older builds stored the requested absolute point even when
                // docking later pinned that axis to the screen edge.
                match memory.dock_edge {
                    Some(crate::PanelEdge::Left)=>memory.horizontal_ratio=0.0,
                    Some(crate::PanelEdge::Right)=>memory.horizontal_ratio=1.0,
                    Some(crate::PanelEdge::Top)=>memory.vertical_ratio=0.0,
                    None=>{},
                }
            }
            memory
        });
    Some((target.clone(), memory))
}

/// User-directed display switches restore that display's memory, or carry the
/// source ratios on the first visit. Unlike hot-plug fallback, this changes the
/// preferred id because the user chose another display.
pub fn switch_display(settings: &mut crate::settings::PanelSettings, current: &Display, target: &Display,
    origin: (i32, i32), rail: (f64, f64)) -> crate::settings::DisplayPlacement {
    if settings.display.as_deref() == Some(current.id.as_str()) || settings.display.is_none() {
        remember(settings, current, origin, rail);
    }
    let carried = settings.per_display.get(&current.id).cloned()
        .or_else(|| settings.display.as_ref().and_then(|id| settings.per_display.get(id)).cloned())
        .unwrap_or_else(|| {
            let (horizontal_ratio, vertical_ratio) = ratios(origin, current, rail);
            crate::settings::DisplayPlacement { dock_edge: settings.dock_edge, facing_edge: settings.facing_edge, horizontal_ratio, vertical_ratio }
        });
    let memory = settings.per_display.entry(target.id.clone()).or_insert(carried).clone();
    settings.display = Some(target.id.clone());
    memory
}

/// Uses the monitor device interface identity instead of \.
/// DISPLAY numbering, which Windows can reassign when monitors reconnect.
#[cfg(windows)]
pub fn display_identity(name: Option<&str>, bounds: Rect) -> String {
    #[repr(C)]
    struct DisplayDevice { size: u32, name: [u16; 32], description: [u16; 128], flags: u32, id: [u16; 128], key: [u16; 128] }
    impl DisplayDevice {
        fn new() -> Self { Self { size: std::mem::size_of::<Self>() as u32, name: [0; 32], description: [0; 128], flags: 0, id: [0; 128], key: [0; 128] } }
    }
    #[link(name = "user32")]
    extern "system" { fn EnumDisplayDevicesW(name: *const u16, index: u32, device: *mut DisplayDevice, flags: u32) -> i32; }
    fn string(raw: &[u16]) -> String { String::from_utf16_lossy(&raw[..raw.iter().position(|value| *value == 0).unwrap_or(raw.len())]) }
    if let Some(name) = name {
        let mut index = 0;
        loop {
            let mut adapter = DisplayDevice::new();
            if unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut adapter, 0) } == 0 { break; }
            index += 1;
            if !string(&adapter.name).eq_ignore_ascii_case(name) { continue; }
            let mut monitor_index = 0;
            loop {
                let mut monitor = DisplayDevice::new();
                if unsafe { EnumDisplayDevicesW(adapter.name.as_ptr(), monitor_index, &mut monitor, 1) } == 0 { break; }
                monitor_index += 1;
                if monitor.flags & 1 == 0 { continue; }
                let identity = string(&monitor.id);
                if !identity.is_empty() { return format!("win-device:{}", identity.to_ascii_lowercase()); }
                let key = string(&monitor.key);
                if !key.is_empty() { return format!("win-registry:{}", key.to_ascii_lowercase()); }
            }
        }
    }
    fallback_identity(name, bounds)
}

#[cfg(not(windows))]
pub fn display_identity(name: Option<&str>, bounds: Rect) -> String { fallback_identity(name, bounds) }

fn fallback_identity(name: Option<&str>, bounds: Rect) -> String {
    // Explicitly marked best effort: virtual/RDP drivers may expose no monitor
    // device identity, and their GDI names or geometry can change on reconnect.
    match name.filter(|name| !name.is_empty()) {
        Some(name) => format!("fallback-name:{}", name.to_ascii_lowercase()),
        None => format!("fallback-geometry:{}:{}:{}:{}", bounds.left, bounds.top, bounds.right, bounds.bottom),
    }
}
#[cfg(test)]mod tests{
    use super::*;
    fn display(id:&str,bounds:Rect,scale:f64)->Display{Display{id:id.into(),bounds,work_area:bounds,scale}}
    #[test]fn fullscreen_requires_full_monitor_without_a_caption(){
        let screen=Rect{left:-1920,top:0,right:0,bottom:1080};
        assert!(covers_display(screen,screen,0,"Chrome_WidgetWin_1",false));
        assert!(!covers_display(Rect{bottom:1040,..screen},screen,0,"Chrome_WidgetWin_1",false));
        assert!(!covers_display(screen,screen,0x00c00000,"App",false));
        assert!(!covers_display(screen,screen,0,"WorkerW",false));
        assert!(!covers_display(screen,screen,0,"App",true));
        assert!(!covers_display(Rect{left:0,right:1920,..screen},screen,0,"App",false));
    }
    #[test]fn moving_display_preserves_relative_position_across_dpi(){
        let old=Rect{left:0,top:0,right:1920,bottom:1080};let new=Rect{left:-2560,top:-100,right:0,bottom:1340};
        assert_eq!(transfer((928,440),old,new,(64.,200.),1.,2.),(-1344,420));
        assert_eq!(transfer((-500,-800),old,new,(64.,200.),1.,2.),(-2560,-100));
    }
    #[test]fn dock_float_keeps_ring_under_hand_and_top_turn_centres(){
        let screen=Rect{left:0,top:0,right:1920,bottom:1040};
        let floated=drag_landing((900.,500.),(32.,300.),(64.,678.),crate::PanelEdge::Right,true,screen,1.,24.);
        assert!(!floated.docked);assert_eq!(floated.grab,(32.,276.));assert_eq!(floated.origin,(868,224));
        let top=drag_landing((900.,10.),floated.grab,(64.,630.),floated.edge,false,screen,1.,24.);
        assert!(top.docked);assert_eq!(top.edge,crate::PanelEdge::Top);assert_eq!(top.grab,(339.,32.));assert_eq!(top.origin,(561,0));
    }
    #[test]fn returning_to_each_display_restores_its_own_dock_and_ratios(){
        let a=display("physical-a",Rect{left:0,top:0,right:1920,bottom:1080},1.);
        let b=display("physical-b",Rect{left:-2560,top:-100,right:0,bottom:1340},2.);
        let mut settings=crate::settings::PanelSettings{dock_edge:None,facing_edge:Some(crate::PanelEdge::Left),..Default::default()};
        remember(&mut settings,&a,(300,200),(64.,200.));
        let saved_a=settings.per_display[&a.id].clone();
        settings.per_display.insert(b.id.clone(),crate::settings::DisplayPlacement{dock_edge:Some(crate::PanelEdge::Top),horizontal_ratio:0.25,vertical_ratio:0.4,..Default::default()});
        let memory_b=switch_display(&mut settings,&a,&b,(300,200),(64.,200.));
        assert_eq!(memory_b.dock_edge,Some(crate::PanelEdge::Top));
        let rail_b=rail_for_memory((64.,200.),crate::PanelEdge::Left,false,&memory_b,24.);
        let origin_b=restored_origin(&memory_b,&b,rail_b);
        assert_eq!(rail_b,(248.,64.));assert_eq!(origin_b,(-2044,-100));
        settings.dock_edge=memory_b.dock_edge;settings.facing_edge=Some(crate::PanelEdge::Top);
        let memory_a=switch_display(&mut settings,&b,&a,origin_b,rail_b);
        assert_eq!(memory_a,saved_a);
        assert_eq!(restored_origin(&memory_a,&a,(64.,200.)),(300,200));
        assert_eq!(settings.display.as_deref(),Some("physical-a"));
    }
    #[test]fn unplug_fallback_keeps_disconnected_memory_and_reconnect_restores_it(){
        let a=display("physical-a",Rect{left:0,top:0,right:1920,bottom:1080},1.);
        let b=display("physical-b",Rect{left:-2560,top:0,right:0,bottom:1440},2.);
        let mut settings=crate::settings::PanelSettings{dock_edge:Some(crate::PanelEdge::Left),facing_edge:Some(crate::PanelEdge::Left),..Default::default()};
        remember(&mut settings,&b,(-2560,520),(64.,200.));
        let original=settings.per_display[&b.id].clone();
        let (fallback,fallback_memory)=resolve(&settings,&[a.clone()],Some("physical-a"),(64.,200.)).unwrap();
        assert_eq!(fallback.id,a.id);assert_eq!(fallback_memory,original);
        settings.rail_position=Some(restored_origin(&fallback_memory,&fallback,(64.,200.)));
        assert_eq!(settings.display.as_deref(),Some("physical-b"));
        let moved_b=display("physical-b",Rect{left:1920,top:100,right:4480,bottom:1540},2.);
        let (restored,memory)=resolve(&settings,&[a,moved_b.clone()],Some("physical-a"),(64.,200.)).unwrap();
        assert_eq!(restored.id,b.id);assert_eq!(memory,original);
        assert_eq!(restored_origin(&memory,&moved_b,(64.,200.)),(1920,620));
        assert_eq!(settings.per_display[&b.id],original);
    }
    #[test]fn mixed_dpi_uses_target_scale_and_clamps_oversized_rails(){
        let a=display("a",Rect{left:0,top:0,right:1920,bottom:1080},1.);
        let b=display("b",Rect{left:-2560,top:-100,right:0,bottom:1340},2.);
        let (horizontal_ratio,vertical_ratio)=ratios((928,440),&a,(64.,200.));
        let memory=crate::settings::DisplayPlacement{dock_edge:None,horizontal_ratio,vertical_ratio,..Default::default()};
        assert_eq!(restored_origin(&memory,&b,(64.,200.)),(-1344,420));
        let tiny=display("tiny",Rect{left:-100,top:20,right:0,bottom:120},3.);
        assert_eq!(restored_origin(&memory,&tiny,(64.,200.)),(-100,20));
        assert_eq!(ratios((-100,20),&tiny,(64.,200.)),(0.5,0.5));
        let invalid=crate::settings::DisplayPlacement{horizontal_ratio:f64::INFINITY,vertical_ratio:f64::NAN,..memory};
        assert_eq!(restored_origin(&invalid,&b,(64.,200.)),(-1344,420));
    }
    #[test]fn legacy_absolute_position_adopts_the_correct_negative_coordinate_display(){
        let a=display("a",Rect{left:0,top:0,right:1920,bottom:1080},1.);
        let b=display("b",Rect{left:-1920,top:0,right:0,bottom:1080},1.);
        let settings=crate::settings::PanelSettings{dock_edge:Some(crate::PanelEdge::Left),rail_position:Some((-1920,300)),facing_edge:Some(crate::PanelEdge::Left),..Default::default()};
        let (chosen,memory)=resolve(&settings,&[a,b.clone()],Some("a"),(64.,200.)).unwrap();
        assert_eq!(chosen.id,"b");assert_eq!(restored_origin(&memory,&b,(64.,200.)),(-1920,300));
    }
    #[test]fn same_display_metrics_keep_the_anchor_from_empty_startup(){
        // Captured native smoke: DPI 1.5, [720,80], an empty 40-point rail
        // growing to seven accounts (630pt). Recalling its temporary ratio
        // used to turn 80 into 45 despite the display never changing.
        let mut screen=display("physical",Rect{left:0,top:0,right:3840,bottom:2160},1.5);
        screen.work_area.bottom=2082;
        let mut settings=crate::settings::PanelSettings{dock_edge:None,facing_edge:Some(crate::PanelEdge::Right),rail_position:Some((720,80)),..Default::default()};
        let (_,empty)=resolve(&settings,&[screen.clone()],Some("physical"),(64.,40.)).unwrap();
        let initial=layout_origin(&settings,&empty,&screen,(64.,40.));
        record_layout(&mut settings,&screen,&empty,initial,(64.,40.));
        assert_eq!(restored_origin(&empty,&screen,(64.,630.)),(720,45));
        for rail in [(64.,630.),(64.,710.),(76.8,756.),(64.,40.),(64.,630.)] {
            let (_,memory)=resolve(&settings,&[screen.clone()],Some("physical"),rail).unwrap();
            let origin=layout_origin(&settings,&memory,&screen,rail);
            assert_eq!(origin,(720,80));
            record_layout(&mut settings,&screen,&memory,origin,rail);
            let saved=&settings.per_display[&screen.id];
            assert_eq!(restored_origin(saved,&screen,rail),origin);
        }
    }
    #[test]fn same_display_metrics_clamp_only_real_overflow(){
        let screen=display("physical",Rect{left:0,top:0,right:1920,bottom:1080},1.);
        let mut settings=crate::settings::PanelSettings{dock_edge:None,facing_edge:Some(crate::PanelEdge::Left),..Default::default()};
        remember(&mut settings,&screen,(300,850),(64.,200.));
        let memory=settings.per_display[&screen.id].clone();
        assert_eq!(layout_origin(&settings,&memory,&screen,(64.,210.)),(300,850));
        assert_eq!(layout_origin(&settings,&memory,&screen,(64.,400.)),(300,680));
        assert_eq!(layout_origin(&settings,&memory,&screen,(2000.,2000.)),(0,0));
        let top=crate::settings::DisplayPlacement{dock_edge:Some(crate::PanelEdge::Top),..memory};
        assert_eq!(layout_origin(&settings,&top,&screen,(500.,64.)),(300,0));
    }
    #[test]fn display_geometry_and_dpi_changes_recall_ratios_instead_of_absolute_pixels(){
        let screen=display("physical",Rect{left:0,top:0,right:1920,bottom:1080},1.);
        let mut settings=crate::settings::PanelSettings{dock_edge:None,facing_edge:Some(crate::PanelEdge::Left),..Default::default()};
        remember(&mut settings,&screen,(300,200),(64.,200.));
        let memory=settings.per_display[&screen.id].clone();
        for changed in [Display{scale:2.,..screen.clone()},Display{work_area:Rect{bottom:900,..screen.work_area},..screen.clone()},Display{bounds:Rect{left:1920,right:3840,..screen.bounds},work_area:Rect{left:1920,right:3840,..screen.work_area},..screen.clone()}] {
            assert!(absolute_anchor(&settings,&changed).is_none());
            assert_eq!(layout_origin(&settings,&memory,&changed,(64.,200.)),restored_origin(&memory,&changed,(64.,200.)));
        }
    }
    #[test]fn fallback_layout_keeps_its_anchor_without_overwriting_the_unplugged_display(){
        let a=display("a",Rect{left:0,top:0,right:1920,bottom:1080},1.);
        let b=display("b",Rect{left:-2560,top:0,right:0,bottom:1440},2.);
        let mut settings=crate::settings::PanelSettings{dock_edge:Some(crate::PanelEdge::Left),facing_edge:Some(crate::PanelEdge::Left),..Default::default()};
        remember(&mut settings,&b,(-2560,520),(64.,200.));
        let original=settings.per_display[&b.id].clone();
        let (_,memory)=resolve(&settings,&[a.clone()],Some("a"),(64.,200.)).unwrap();
        let origin=layout_origin(&settings,&memory,&a,(64.,200.));
        record_layout(&mut settings,&a,&memory,origin,(64.,200.));
        assert_eq!(layout_origin(&settings,&memory,&a,(64.,300.)),origin);
        assert_eq!(settings.per_display[&b.id],original);assert_eq!(settings.display.as_deref(),Some("b"));
        assert!(absolute_anchor(&settings,&b).is_none());
        assert_eq!(layout_origin(&settings,&original,&b,(64.,200.)),(-2560,520));
    }
    #[test]fn top_uses_physical_edge_while_sides_use_the_work_area(){
        let mut screen=display("screen",Rect{left:0,top:0,right:1920,bottom:1080},1.);
        screen.work_area=Rect{left:40,top:30,right:1920,bottom:1040};
        let top=crate::settings::DisplayPlacement{dock_edge:Some(crate::PanelEdge::Top),horizontal_ratio:0.5,..Default::default()};
        assert_eq!(restored_origin(&top,&screen,(200.,64.)),(880,0));
        let left=crate::settings::DisplayPlacement{dock_edge:Some(crate::PanelEdge::Left),..Default::default()};
        assert_eq!(restored_origin(&left,&screen,(64.,200.)),(40,435));
    }
    #[test]fn drag_keeps_the_logical_grab_offset_across_dpi(){
        let screen=Rect{left:0,top:0,right:1920,bottom:1080};
        let landing=drag_landing((900.,500.),(32.,100.),(64.,300.),crate::PanelEdge::Right,false,screen,2.,24.);
        assert!(!landing.docked);assert_eq!(landing.grab,(32.,100.));assert_eq!(landing.origin,(836,300));
        assert_eq!(landing.rail,(64.,300.));
    }
    #[test]fn fallback_identity_is_explicit_and_not_resolution_based_when_a_name_exists(){
        let first=Rect{left:0,top:0,right:1920,bottom:1080};let changed=Rect{left:-2560,top:0,right:0,bottom:1440};
        assert_eq!(fallback_identity(Some("Virtual-1"),first),fallback_identity(Some("virtual-1"),changed));
        assert!(fallback_identity(Some("virtual-1"),first).starts_with("fallback-name:"));
        assert!(fallback_identity(None,first).starts_with("fallback-geometry:"));
    }
}
