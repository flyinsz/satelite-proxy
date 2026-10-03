/** Node-list layout constants shared by NodesPage (main list / grid views) and
 *  PoolsView (pool member rows).
 *
 *  Kept in one module so a column change in the list view can never desync the
 *  pool rows — the two render the same grid template and would otherwise drift
 *  apart silently. */

/** Slim group header band height (px). */
export const NODE_GROUP_H = 36;

/** List view column template — shared by the head row and every data row so
 *  they align without relying on native <table> auto-layout (dropped so the
 *  group header row can span full width and grow past a single line). */
export const NODE_LIST_COLS = "40px minmax(0,1.44fr) 90px minmax(0,1fr) 70px 90px";