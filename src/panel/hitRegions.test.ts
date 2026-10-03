import { describe, expect, it } from 'vitest'
import { dockLayout, type PanelEdge } from './berthShape'
import { detailCardLayout } from './cardLayout'
import { cardSurfaceRegion, cardTrackingRegion, outlineContains, pathOutline, pointerHits, railSurfaceRegion, regionContains, sliverTrackingRegion, type RailHitGeometry, type ShapeHitRegion } from './hitRegions'

const rail=(patch:Partial<RailHitGeometry>={}):RailHitGeometry=>({edge:'right',left:300,top:20,width:64,length:400,layout:dockLayout(),openness:1,isDocked:false,...patch})
const card=(edge:PanelEdge='right',usesRoundEnds=false)=>cardSurfaceRegion({edge,left:10,top:20,width:edge==='top'?250:270,height:edge==='top'?200:180,pointerCentre:edge==='top'?125:90,layout:detailCardLayout(),usesRoundEnds})

describe('native outline geometry',()=>{
  it('preserves the original nonzero fill rule in the body/tail overlap',()=>{
    const outline=pathOutline('M 0 0 L 10 0 L 10 20 L 0 20 Z M 9 8 L 15 10 L 9 12 Z')
    expect(outline).toHaveLength(2)
    expect(outlineContains(outline,9.5,10)).toBe(true)
    expect(outlineContains(outline,14,10)).toBe(true)
    expect(outlineContains(outline,14,5)).toBe(false)
    expect(outlineContains(outline,10,15)).toBe(true)
    expect(outlineContains(outline,NaN,5)).toBe(false)
  })

  it('matches circular floating caps, excluding transparent corners',()=>{
    const surface=railSurfaceRegion(rail()),radius=32
    for(let x=.5;x<64;x+=2)for(let y=.5;y<400;y+=2){
      const centreY=y<radius?radius:y>400-radius?400-radius:y
      const distance=Math.hypot(x-radius,y-centreY)
      // The sent contour has less than .04pt sagitta. Avoid sampling its
      // subpixel edge when comparing against the analytic original circle.
      if(Math.abs(distance-radius)<.08)continue
      expect(regionContains(surface,[surface],300+x,20+y),`capsule ${x},${y}`).toBe(distance<radius)
    }
    expect(regionContains(surface,[surface],301,21)).toBe(false)
    expect(regionContains(surface,[surface],332,21)).toBe(true)
    expect(regionContains(surface,[surface],332,419)).toBe(true)
  })

  it('mirrors and rotates the very same docked flare at all three edges',()=>{
    for(const round of [false,true]){
      const layout=dockLayout({usesRoundEnds:round})
      const right=railSurfaceRegion(rail({edge:'right',isDocked:true,layout})),left=railSurfaceRegion(rail({edge:'left',isDocked:true,layout})),top=railSurfaceRegion(rail({edge:'top',isDocked:true,layout}))
      for(let x=1.3;x<64;x+=5)for(let y=1.7;y<400;y+=7){
        const expected=regionContains(right,[right],300+x,20+y)
        expect(regionContains(left,[left],300+64-x,20+y)).toBe(expected)
        expect(regionContains(top,[top],300+y,20+64-x)).toBe(expected)
      }
      expect(regionContains(right,[right],300.5,20.5)).toBe(false)
      expect(regionContains(right,[right],363.9,25.5)).toBe(true)
    }
  })

  it('tracks the interpolated painted dimensions while the rail winds down',()=>{
    for(const openness of [0,.2,.5,.8,1])for(const edge of ['right','left','top'] as const){
      const input=rail({edge,isDocked:true,openness}),surface=railSurfaceRegion(input)
      const width=6+58*openness,length=96+304*openness
      expect(surface.w).toBeCloseTo(edge==='top'?length:width)
      expect(surface.h).toBeCloseTo(edge==='top'?width:length)
      expect(surface.x).toBeCloseTo(edge==='top'?300+(400-length)/2:edge==='right'?300+64-width:300)
      expect(surface.y).toBeCloseTo(edge==='top'?20:20+(400-length)/2)
      for(const contour of surface.outline!)for(const [x,y] of contour){
        expect(x).toBeGreaterThanOrEqual(surface.x-.001);expect(x).toBeLessThanOrEqual(surface.x+surface.w+.001)
        expect(y).toBeGreaterThanOrEqual(surface.y-.001);expect(y).toBeLessThanOrEqual(surface.y+surface.h+.001)
      }
    }
  })

  it('claims the real card tail and squircle while its empty tail band passes through',()=>{
    for(const round of [false,true]){
      const right=card('right',round),left=card('left',round),top=card('top',round)
      expect(right.outline).toHaveLength(2)
      expect(regionContains(right,[right],11,21)).toBe(false)
      expect(regionContains(right,[right],40,21)).toBe(true)
      expect(regionContains(right,[right],279.9,110)).toBe(true)
      expect(regionContains(right,[right],279,70)).toBe(false)
      expect(regionContains(right,[right],259,110)).toBe(true)
      expect(regionContains(left,[left],10.1,110)).toBe(true)
      expect(regionContains(left,[left],11,70)).toBe(false)
      expect(regionContains(top,[top],135,20.1)).toBe(true)
      expect(regionContains(top,[top],50,21)).toBe(false)
      expect(regionContains(top,[top],135,70)).toBe(true)
    }
  })

  it('clamps an extreme tail target inside the body corners at every edge',()=>{
    for(const edge of ['right','left','top'] as const)for(const pointerCentre of [-999,999]){
      const surface=cardSurfaceRegion({edge,left:0,top:0,width:edge==='top'?250:270,height:edge==='top'?200:180,pointerCentre,layout:detailCardLayout(),usesRoundEnds:true})
      const tip=edge==='top'?surface.outline![1].filter(([,y])=>Math.abs(y)<.001):surface.outline![1].filter(([x])=>Math.abs(x-(edge==='right'?270:0))<.001)
      expect(tip).toHaveLength(1)
      const along=edge==='top'?tip[0][0]:tip[0][1]
      expect(along).toBeGreaterThanOrEqual(40)
      expect(along).toBeLessThanOrEqual((edge==='top'?250:180)-40)
    }
  })
})

describe('hover and click ownership protocol',()=>{
  it('retains rail hover at a transparent corner and clips account selection there',()=>{
    const surface=railSurfaceRegion(rail()),tracking:ShapeHitRegion={...surface,outline:undefined,hoverOnly:true},entry:ShapeHitRegion={id:'entry:codex',x:300,y:20,w:64,h:58,clipTo:'rail'}
    const regions=[tracking,surface,entry]
    expect(pointerHits(regions,301,21)).toEqual({hovered:'rail',claimsClick:false})
    expect(pointerHits(regions,332,21)).toEqual({hovered:'entry:codex',claimsClick:true})
    expect(pointerHits(regions,350,200)).toEqual({hovered:'rail',claimsClick:true})
    expect(pointerHits(regions,290,200)).toEqual({hovered:null,claimsClick:false})
  })

  it('the original sliver tracking forgiveness opens without taking a transparent press',()=>{
    for(const edge of ['right','left','top'] as const){
      const input=rail({edge,isDocked:true,openness:0}),tracking=sliverTrackingRegion(input),surface=railSurfaceRegion(input,'sliver')
      const regions=[tracking,surface],empty=edge==='top'?[tracking.x+tracking.w/2,tracking.y+15]:edge==='left'?[tracking.x+15,tracking.y+tracking.h/2]:[tracking.x+2,tracking.y+tracking.h/2]
      const painted=edge==='top'?[surface.x+surface.w/2,surface.y+3]:[surface.x+3,surface.y+surface.h/2]
      expect(pointerHits(regions,empty[0],empty[1])).toEqual({hovered:'sliver',claimsClick:false})
      expect(pointerHits(regions,painted[0],painted[1])).toEqual({hovered:'sliver',claimsClick:true})
      expect(edge==='top'?tracking.w:tracking.h).toBe(112)
    }
  })

  it('retains the card across its gap and eight-point slack without intercepting them',()=>{
    const surface=card(),tracking=cardTrackingRegion('right',342,500,surface),regions=[tracking,surface]
    expect(pointerHits(regions,285,110)).toEqual({hovered:'card',claimsClick:false})
    expect(pointerHits(regions,100,15)).toEqual({hovered:'card',claimsClick:false})
    expect(pointerHits(regions,100,110)).toEqual({hovered:'card',claimsClick:true})
    expect(pointerHits(regions,100,10)).toEqual({hovered:null,claimsClick:false})
    const top=card('top'),topTracking=cardTrackingRegion('top',500,280,top)
    expect(pointerHits([topTracking,top],135,15)).toEqual({hovered:'card',claimsClick:false})
    expect(pointerHits([topTracking,top],135,100)).toEqual({hovered:'card',claimsClick:true})
  })

  it('survives JSON transport, respects native region priority and accepts legacy rectangles',()=>{
    const surface=railSurfaceRegion(rail()),entry:ShapeHitRegion={id:'entry:codex',x:300,y:20,w:64,h:58,clipTo:'rail'},regions=JSON.parse(JSON.stringify([surface,entry])) as ShapeHitRegion[]
    expect(pointerHits(regions,332,21)).toEqual({hovered:'entry:codex',claimsClick:true})
    expect(pointerHits([{id:'legacy',x:1,y:2,w:4,h:6}],3,4)).toEqual({hovered:'legacy',claimsClick:true})
    expect(pointerHits([{...entry,clipTo:'missing'}],332,21)).toEqual({hovered:null,claimsClick:false})
    expect(pointerHits([{...surface,outline:[]}],332,21)).toEqual({hovered:null,claimsClick:false})
    expect(pointerHits([{...surface,outline:[[[0,0],[NaN,20],[30,40]]]}],332,21)).toEqual({hovered:null,claimsClick:false})
    expect(pointerHits(regions,NaN,21)).toEqual({hovered:null,claimsClick:false})
  })
})
