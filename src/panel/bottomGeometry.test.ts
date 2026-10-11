import { describe, it, expect } from 'vitest'
import { berthCanvas, berthTransform, dockLayout } from './berthShape'
import { railSurfaceRegion, cardSurfaceRegion, pointerHits, sliverTrackingRegion } from './hitRegions'
import { cardBox, detailCardLayout } from './cardLayout'

describe('bottom-facing panel geometry', () => {
  it('uses horizontal canvas and keeps the collapsed click surface at the bottom', () => {
    expect(berthCanvas('bottom',64,360)).toEqual({width:360,height:64})
    expect(berthTransform('bottom',360,64)).toBe('translate(360 0) rotate(90)')
    const geometry={edge:'bottom' as const,left:100,top:200,width:64,length:360,layout:dockLayout({}),openness:0,isDocked:true}
    const surface=railSurfaceRegion(geometry),tracking=sliverTrackingRegion(geometry)
    expect(surface.y+surface.h).toBe(264)
    expect(tracking.y+tracking.h).toBe(264)
    expect(pointerHits([tracking,surface],surface.x+surface.w/2,263).claimsClick).toBe(true)
    expect(pointerHits([tracking,surface],surface.x+surface.w/2,200).claimsClick).toBe(false)
  })
  it('places the bottom card tail below its body with the requested horizontal anchor',()=>{
    const layout=detailCardLayout('standard',false,false),box=cardBox('bottom',layout,1)
    expect(box.width).toBe(layout.width)
    const surface=cardSurfaceRegion({edge:'bottom',left:0,top:0,...box,pointerCentre:60,layout,usesRoundEnds:false})
    expect(pointerHits([surface],60,box.height-1).claimsClick).toBe(true)
    expect(pointerHits([surface],box.width-60,box.height-1).claimsClick).toBe(false)
    expect(pointerHits([surface],box.width/2,layout.padding*2).claimsClick).toBe(true)
  })
})
