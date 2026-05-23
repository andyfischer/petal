/** Render an array of draw-command JSON objects to a Canvas2D context. */

export interface DrawCommand {
  op: string;
  // Color fields (shared by all commands)
  r?: number;
  g?: number;
  b?: number;
  // Rect / RectOutline
  x?: number;
  y?: number;
  w?: number;
  h?: number;
  // Line
  x1?: number;
  y1?: number;
  x2?: number;
  y2?: number;
  // Circle
  cx?: number;
  cy?: number;
  radius?: number;
  // Text
  text?: string;
  size?: number;
}

function rgb(r: number, g: number, b: number): string {
  return `rgb(${r},${g},${b})`;
}

export function renderCommands(
  ctx: CanvasRenderingContext2D,
  commands: DrawCommand[],
  canvasWidth: number,
  canvasHeight: number,
): void {
  for (const cmd of commands) {
    switch (cmd.op) {
      case "clear":
        ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
        ctx.fillRect(0, 0, canvasWidth, canvasHeight);
        break;

      case "rect":
        ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
        ctx.fillRect(cmd.x!, cmd.y!, cmd.w!, cmd.h!);
        break;

      case "rect_outline":
        ctx.strokeStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
        ctx.lineWidth = 1;
        ctx.strokeRect(cmd.x!, cmd.y!, cmd.w!, cmd.h!);
        break;

      case "line":
        ctx.strokeStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(cmd.x1!, cmd.y1!);
        ctx.lineTo(cmd.x2!, cmd.y2!);
        ctx.stroke();
        break;

      case "circle":
        ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
        ctx.beginPath();
        ctx.arc(cmd.cx!, cmd.cy!, Math.abs(cmd.radius!), 0, Math.PI * 2);
        ctx.fill();
        break;

      case "text":
        ctx.fillStyle = rgb(cmd.r!, cmd.g!, cmd.b!);
        ctx.font = `${cmd.size}px sans-serif`;
        ctx.textBaseline = "top";
        ctx.fillText(cmd.text!, cmd.x!, cmd.y!);
        break;
    }
  }
}
