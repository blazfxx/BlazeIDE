// Injected into HTML pages served to the live preview. Lets the user click an
// element and attach a comment for the AI; talks to the IDE via postMessage.
(() => {
  if (window.__voidSel) return; window.__voidSel = true;
  var selecting = false, hovered = null;
  var HL = document.createElement('div');
  HL.style.cssText = 'position:fixed;z-index:2147483646;pointer-events:none;border:2px solid #4af;background:rgba(68,170,255,0.14);border-radius:2px;display:none;';
  var tip = document.createElement('div');
  tip.style.cssText = 'position:fixed;z-index:2147483647;pointer-events:none;font:11px ui-monospace,monospace;background:#111;color:#eee;padding:2px 6px;border-radius:4px;display:none;';
  var ready = function(){ document.documentElement.appendChild(HL); document.documentElement.appendChild(tip); };
  if (document.body) ready(); else document.addEventListener('DOMContentLoaded', ready);

  function sel(e){ if(e.id) return '#'+e.id; var parts=[],n=e;
    while(n&&n.nodeType===1&&parts.length<4&&n.tagName!=='BODY'){ var p=n.tagName.toLowerCase();
      if(n.classList&&n.classList.length) p+='.'+Array.prototype.slice.call(n.classList,0,2).join('.');
      var par=n.parentElement; if(par){ var sibs=Array.prototype.filter.call(par.children,function(c){return c.tagName===n.tagName;});
        if(sibs.length>1) p+=':nth-of-type('+(sibs.indexOf(n)+1)+')'; }
      parts.unshift(p); n=n.parentElement; } return parts.join(' > '); }

  function place(e){ var r=e.getBoundingClientRect(); HL.style.display='block';
    HL.style.left=r.left+'px'; HL.style.top=r.top+'px'; HL.style.width=r.width+'px'; HL.style.height=r.height+'px';
    tip.style.display='block'; tip.textContent=e.tagName.toLowerCase()+(e.id?'#'+e.id:'')+(e.classList&&e.classList.length?'.'+Array.prototype.join.call(e.classList,'.'):'');
    tip.style.left=r.left+'px'; tip.style.top=Math.max(0,r.top-20)+'px'; }
  function clearHL(){ HL.style.display='none'; tip.style.display='none'; }

  document.addEventListener('mousemove', function(ev){ if(!selecting) return; var t=ev.target;
    if(t&&t.nodeType===1){ hovered=t; place(t); } }, true);
  document.addEventListener('mouseover', function(ev){ if(!selecting||boxOpen) return; var t=ev.target;
    if(t&&t.nodeType===1){ hovered=t; place(t); } }, true);
  document.addEventListener('click', function(ev){ if(!selecting||boxOpen) return;
    ev.preventDefault(); ev.stopPropagation(); var t=ev.target||hovered; if(t&&t.nodeType===1) box(t); }, true);

  var boxOpen = false;
  function box(el){
    if(boxOpen) return;
    boxOpen = true;
    clearHL();
    var ov=document.createElement('div');
    ov.style.cssText='position:fixed;inset:0;z-index:2147483647;background:rgba(0,0,0,0.4);display:flex;align-items:center;justify-content:center;';
    var card=document.createElement('div');
    card.style.cssText='background:#1b1b1b;color:#eee;border:1px solid #333;border-radius:8px;padding:14px;width:min(420px,90vw);font:13px ui-sans-serif,system-ui;box-shadow:0 8px 30px rgba(0,0,0,.5);';
    var label=el.tagName.toLowerCase()+(el.id?'#'+el.id:'');
    var lab=document.createElement('div'); lab.setAttribute('style','font:12px ui-sans-serif,system-ui!important;color:#9aa!important;margin:0 0 8px!important;'); lab.textContent='Comment on '+label; card.appendChild(lab);
    var ta=document.createElement('textarea'); ta.placeholder='What should change about this element?';
    ta.setAttribute('style','all:revert;width:100%!important;box-sizing:border-box!important;height:72px!important;resize:vertical!important;background:#0f0f0f!important;color:#eee!important;border:1px solid #333!important;border-radius:6px!important;padding:8px!important;font:13px ui-sans-serif,system-ui!important;outline:none!important;margin:0!important;');
    var row=document.createElement('div'); row.style.cssText='display:flex;gap:8px;justify-content:flex-end;margin-top:10px;';
    // Force styles with !important + reset inherited props, so the previewed
    // page's own button/textarea CSS can't make our controls invisible.
    var btnBase='all:revert;font:13px ui-sans-serif,system-ui!important;border-radius:6px!important;padding:6px 14px!important;cursor:pointer!important;border:1px solid #333!important;margin:0!important;';
    var cancel=document.createElement('button'); cancel.textContent='Cancel'; cancel.setAttribute('style',btnBase+'background:#2a2a2a!important;color:#eee!important;');
    var add=document.createElement('button'); add.textContent='Add comment'; add.setAttribute('style',btnBase+'background:#2a6df0!important;color:#fff!important;border-color:#2a6df0!important;');
    row.appendChild(cancel); row.appendChild(add); card.appendChild(ta); card.appendChild(row); ov.appendChild(card);
    document.documentElement.appendChild(ov); ta.focus();
    function close(){ ov.remove(); boxOpen = false; }
    cancel.addEventListener('click', function(e){ e.stopPropagation(); close(); });
    ov.addEventListener('click', function(e){ if(e.target===ov) close(); });
    ta.addEventListener('keydown', function(e){ if(e.key==='Enter'&&(e.metaKey||e.ctrlKey)) add.click(); });
    add.addEventListener('click', function(e){ e.stopPropagation(); var text=ta.value.trim(); if(!text){ ta.focus(); return; }
      // Capture only the element's OPENING tag (tag + attributes), not its
      // children — so commenting on a container (e.g. <body>) doesn't dump the
      // whole page and mislead the AI into editing a child element.
      var openTag='';
      try {
        openTag='<'+el.tagName.toLowerCase();
        if(el.attributes){ for(var i=0;i<el.attributes.length;i++){ var a=el.attributes[i];
          if(a.name==='style'&&el.getAttribute('style')&&el.getAttribute('style').indexOf('cursor: crosshair')>=0){
            // strip the crosshair we added in select mode
            var cleaned=a.value.replace(/cursor:\s*crosshair;?/,'').trim(); if(cleaned) openTag+=' style="'+cleaned+'"';
          } else { openTag+=' '+a.name+(a.value?'="'+a.value+'"':''); } } }
        openTag+='>';
        openTag=openTag.slice(0,300);
      } catch(err){ openTag='<'+el.tagName.toLowerCase()+'>'; }
      // Direct text of THIS element only (not nested), trimmed.
      var txt='';
      try { var d=''; for(var k=0;k<el.childNodes.length;k++){ var cn=el.childNodes[k]; if(cn.nodeType===3) d+=cn.textContent; }
        txt=d.trim().slice(0,100); } catch(e2){}
      parent.postMessage({type:'void:element-comment',comment:{
        comment:text, selector:sel(el), tag:el.tagName.toLowerCase(),
        text:txt, html:openTag, pageUrl:location.href }},'*');
      close();
    });
  }

  window.addEventListener('message', function(ev){ var d=ev.data; if(!d||d.type!=='void:set-selecting') return;
    selecting=!!d.on; if(document.body) document.body.style.cursor=selecting?'crosshair':''; if(!selecting) clearHL(); });
})();
