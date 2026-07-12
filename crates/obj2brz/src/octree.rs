use cgmath::Vector3;
/**
 * Copyright (C) 2014 Ben Foppa
 * Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
 * The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
*/
use std::mem;

pub struct VoxelTree<T> {
    pub size: u8,
    pub contents: Branches<T>,
}

#[repr(C)]
pub enum TreeBody<T> {
    Empty,
    Leaf(T),
    Branch(Box<Branches<T>>),
}

pub type Branches<T> = [TreeBody<T>; 8];

impl<T> TreeBody<T> {
    pub fn empty() -> Branches<T> {
        [
            TreeBody::Empty,
            TreeBody::Empty,
            TreeBody::Empty,
            TreeBody::Empty,
            TreeBody::Empty,
            TreeBody::Empty,
            TreeBody::Empty,
            TreeBody::Empty,
        ]
    }
}

impl<T> VoxelTree<T> {
    pub fn new() -> VoxelTree<T> {
        VoxelTree {
            size: 0,
            contents: TreeBody::empty(),
        }
    }

    pub fn contains_bounds(&self, voxel: Vector3<isize>) -> bool {
        let high = 1 << self.size;
        let low = -high;

        if voxel.x < low || voxel.y < low || voxel.z < low {
            return false;
        }

        (voxel.x + 1) <= high && (voxel.y + 1) <= high && (voxel.z + 1) <= high
    }

    pub fn grow_to_hold(&mut self, voxel: Vector3<isize>) {
        while !self.contains_bounds(voxel) {
            self.size += 1;

            // We re-construct the tree with bounds twice the size (but still centered
            // around the origin) by deconstructing the top level of branches,
            // creating a new doubly-sized top level, and moving the old branches back
            // in as the new top level's children. e.g. in 2D:
            //
            //                      ---------------------------
            //                      |     |     |0|     |     |
            //                      |     |     |0|     |     |
            // ---------------      ------------|0|------------
            // |  1  |0|  2  |      |     |  1  |0|  2  |     |
            // |     |0|     |      |     |     |0|     |     |
            // |------0------|      |------------0------------|
            // 000000000000000  ==> |0000000000000000000000000|
            // |------0------|      |------------0------------|
            // |     |0|     |      |     |     |0|     |     |
            // |  3  |0|  4  |      |     |  3  |0|  4  |     |
            // ---------------      |------------0------------|
            //                      |     |     |0|     |     |
            //                      |     |     |0|     |     |
            //                      ---------------------------

            macro_rules! at(
                ($c_idx:literal, $b_idx:literal) => {{
                    let mut branches = TreeBody::empty();
                    branches[$b_idx] = mem::replace(&mut self.contents[$c_idx], TreeBody::Empty);
                    TreeBody::Branch(Box::new(branches))
                }}
            );

            self.contents = [
                at!(0, 7),
                at!(1, 6),
                at!(2, 5),
                at!(3, 4),
                at!(4, 3),
                at!(5, 2),
                at!(6, 1),
                at!(7, 0),
            ];
        }
    }

    pub fn get_mut_or_create(&mut self, voxel: Vector3<isize>) -> &mut TreeBody<T> {
        self.grow_to_hold(voxel);
        let mut m = 1 << self.size;
        let mut branch = &mut self.contents[(((voxel.x >= 0) as usize) << 2)
            + (((voxel.y >= 0) as usize) << 1)
            + (voxel.z >= 0) as usize];

        loop {
            m >>= 1;
            if m == 0 {
                return branch;
            }

            let branch_id = ((((voxel.x & m) != 0) as usize) << 2)
                + ((((voxel.y & m) != 0) as usize) << 1)
                + ((voxel.z & m) != 0) as usize;

            match branch {
                TreeBody::Branch(b) => {
                    branch = &mut b[branch_id];
                }
                _ => {
                    // Make branch into a TreeBody::Branch if it isint
                    *branch = TreeBody::Branch(Box::new(TreeBody::empty()));

                    match branch {
                        TreeBody::Branch(b) => {
                            branch = &mut b[branch_id];
                        }
                        _ => unreachable!(),
                    }
                }
            };
        }
    }

    pub fn get_any_mut_or_create(&mut self) -> (Vector3<isize>, &mut TreeBody<T>) {
        let mask = 1 << self.size;
        let voxel = Vector3::<isize>::new(-mask, -mask, -mask);

        let branches = &mut self.contents;
        match VoxelTree::get_any_recursive(branches, mask, voxel) {
            Some(vector) => (vector, self.get_mut_or_create(vector)),
            None => (
                voxel,
                self.get_mut_or_create(Vector3::<isize>::new(0, 0, 0)),
            ),
        }
    }

    fn get_any_recursive(
        branches: &mut Branches<T>,
        mask: isize,
        voxel: Vector3<isize>,
    ) -> Option<Vector3<isize>> {
        let m = mask >> 1;

        for (i, branch) in branches.iter_mut().enumerate() {
            let mut voxel_temp = voxel;
            let step = 2 * m + ((m == 0) as isize);
            voxel_temp.x += step * ((i & 4) > 0) as isize;
            voxel_temp.y += step * ((i & 2) > 0) as isize;
            voxel_temp.z += step * ((i & 1) > 0) as isize;

            match branch {
                TreeBody::Branch(b) => {
                    if m == 0 {
                        continue;
                    } // Dont recurse further

                    let new_vector = VoxelTree::get_any_recursive(b, m, voxel_temp);
                    match new_vector {
                        Some(vector) => {
                            return Some(vector);
                        }
                        None => {
                            // No valid points in branch, clean up...
                            *branch = TreeBody::Empty;
                        }
                    }
                }
                TreeBody::Leaf(_) => {
                    if m == 0 {
                        return Some(voxel_temp);
                    }
                }
                TreeBody::Empty => {}
            }
        }

        // No valid points in branch
        None
    }
}
