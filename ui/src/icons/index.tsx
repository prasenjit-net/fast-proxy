// The template's own icon pack: hand-drawn 24×24 stroke icons sharing
// one visual grammar (1.75px stroke, round caps/joins). Add icons by
// appending another `icon(<>…</>)` — no external icon library needed.
import type { ReactElement, ReactNode, SVGProps } from "react";

export type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function icon(children: ReactNode) {
  return function Icon({ size = 20, ...props }: IconProps): ReactElement {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={1.75}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        {...props}
      >
        {children}
      </svg>
    );
  };
}

export const IconDashboard = icon(
  <>
    <rect x="3" y="3" width="7" height="7" rx="1.5" />
    <rect x="14" y="3" width="7" height="7" rx="1.5" />
    <rect x="3" y="14" width="7" height="7" rx="1.5" />
    <rect x="14" y="14" width="7" height="7" rx="1.5" />
  </>,
);

export const IconLayers = icon(
  <>
    <path d="m12 2.5 9 5-9 5-9-5 9-5z" />
    <path d="m3 12.5 9 5 9-5" />
    <path d="m3 17.5 9 5 9-5" />
  </>,
);

export const IconSliders = icon(
  <>
    <path d="M6 4v16M12 4v16M18 4v16" />
    <path d="M4 14h4M10 8h4M16 16h4" />
  </>,
);

export const IconSun = icon(
  <>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2m0 16v2M4.93 4.93l1.41 1.41m11.32 11.32 1.41 1.41M2 12h2m16 0h2M4.93 19.07l1.41-1.41m11.32-11.32 1.41-1.41" />
  </>,
);

export const IconMoon = icon(
  <path d="M20.5 14.5A8.5 8.5 0 0 1 9.5 3.5a8.5 8.5 0 1 0 11 11z" />,
);

export const IconMonitor = icon(
  <>
    <rect x="3" y="4" width="18" height="13" rx="2" />
    <path d="M9 21h6M12 17v4" />
  </>,
);

export const IconMenu = icon(<path d="M4 7h16M4 12h16M4 17h16" />);

export const IconX = icon(<path d="m6 6 12 12M18 6 6 18" />);

export const IconPlus = icon(<path d="M12 5v14M5 12h14" />);

export const IconTrash = icon(
  <>
    <path d="M4 7h16" />
    <path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
    <path d="m6 7 1 12.5A1.5 1.5 0 0 0 8.5 21h7a1.5 1.5 0 0 0 1.5-1.5L18 7" />
    <path d="M10 11v6M14 11v6" />
  </>,
);

export const IconCheck = icon(<path d="m4.5 12.5 5 5L19.5 7" />);

export const IconAlertTriangle = icon(
  <>
    <path d="M12 3.5 2.5 19.5h19L12 3.5z" />
    <path d="M12 10v4M12 17.4v.01" />
  </>,
);

export const IconInfo = icon(
  <>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 11v5M12 8v.01" />
  </>,
);

export const IconXCircle = icon(
  <>
    <circle cx="12" cy="12" r="9" />
    <path d="m9.5 9.5 5 5m0-5-5 5" />
  </>,
);

export const IconCheckCircle = icon(
  <>
    <circle cx="12" cy="12" r="9" />
    <path d="m8.5 12.5 2.5 2.5 5-5.5" />
  </>,
);

export const IconBolt = icon(<path d="M13 2 4 14h6l-1 8 9-12h-6l1-8z" />);

export const IconActivity = icon(<path d="M3 12h4l2.5-7 4 14L16 12h5" />);

export const IconServer = icon(
  <>
    <rect x="3" y="4" width="18" height="7" rx="2" />
    <rect x="3" y="13" width="18" height="7" rx="2" />
    <path d="M7 7.5h.01M7 16.5h.01" />
  </>,
);

export const IconCpu = icon(
  <>
    <rect x="6" y="6" width="12" height="12" rx="2" />
    <rect x="10" y="10" width="4" height="4" rx="0.5" />
    <path d="M9 2v2.5M15 2v2.5M9 19.5V22M15 19.5V22M2 9h2.5M2 15h2.5M19.5 9H22M19.5 15H22" />
  </>,
);

export const IconDatabase = icon(
  <>
    <ellipse cx="12" cy="5.5" rx="8" ry="3" />
    <path d="M4 5.5V12c0 1.66 3.58 3 8 3s8-1.34 8-3V5.5" />
    <path d="M4 12v6.5c0 1.66 3.58 3 8 3s8-1.34 8-3V12" />
  </>,
);

export const IconUsers = icon(
  <>
    <circle cx="9" cy="8.5" r="3.5" />
    <path d="M3.5 20c.5-3.5 2.8-5.5 5.5-5.5s5 2 5.5 5.5" />
    <circle cx="17" cy="9.5" r="2.75" />
    <path d="M16.5 14.7c2.2.4 3.6 2.1 4 4.8" />
  </>,
);

export const IconWifi = icon(
  <>
    <path d="M2.5 9.5a14.5 14.5 0 0 1 19 0" />
    <path d="M6 13a9.5 9.5 0 0 1 12 0" />
    <path d="M9.5 16.5a4.5 4.5 0 0 1 5 0" />
    <path d="M12 20h.01" />
  </>,
);

export const IconWifiOff = icon(
  <>
    <path d="M2.5 9.5A14.5 14.5 0 0 1 8 6.4m5.5-.8a14.5 14.5 0 0 1 8 3.9" />
    <path d="M6 13a9.5 9.5 0 0 1 3.5-2.2M14 11a9.5 9.5 0 0 1 4 2" />
    <path d="M9.5 16.5a4.5 4.5 0 0 1 5 0" />
    <path d="M12 20h.01" />
    <path d="m3 3 18 18" />
  </>,
);

export const IconClock = icon(
  <>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 7v5l3.5 2" />
  </>,
);

export const IconExternal = icon(
  <>
    <path d="M10 5H6.5A2.5 2.5 0 0 0 4 7.5v10A2.5 2.5 0 0 0 6.5 20h10a2.5 2.5 0 0 0 2.5-2.5V14" />
    <path d="M14 4h6v6M20 4l-9 9" />
  </>,
);

export const IconChevronRight = icon(<path d="m9 5 7 7-7 7" />);
