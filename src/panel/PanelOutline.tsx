/** A one-pixel inside edge of the filled silhouette, including its tail.
 * Eroding the composed alpha avoids outlining the overlapping body/tail seam.
 * The filter never expands paint or changes native hit regions. */
export function PanelOutline({id}:{id:string}) {
  return <filter id={id} x="0" y="0" width="100%" height="100%" colorInterpolationFilters="sRGB">
    <feMorphology in="SourceAlpha" operator="erode" radius="1" result="inside"/>
    <feComposite in="SourceAlpha" in2="inside" operator="out" result="edge"/>
    <feFlood floodColor="#000" floodOpacity=".12" result="ink"/>
    <feComposite in="ink" in2="edge" operator="in" result="outline"/>
    <feMerge><feMergeNode in="SourceGraphic"/><feMergeNode in="outline"/></feMerge>
  </filter>
}
